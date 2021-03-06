mod connection;
mod late_init;
mod utils;


use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, HtmlInputElement, Window, EventTarget, Event, KeyboardEvent};
use tak_garden_common::{ServerMessage, ClientMessage, MatchControl};
use rustak::{
  MoveHistory, Game, GameState, MoveState, MoveAction, WinKind, 
  BoardSize, Location, Direction, Color,
  Stone, StoneKind, StoneStack,
  ActionInvalidReason, PlacementInvalidReason, MovementInvalidReason,
  file_idx_to_char,
};
use connection::Connection;
use late_init::LateInit;
use std::fmt::Write;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[wasm_bindgen]
extern "C" {
  #[wasm_bindgen(js_namespace = console)]
  fn log(s: &str);
}

#[macro_export] macro_rules! console_log {
    // this needs to call crate::log so that if the macro is expanded in other modules it can still find the function
  ($($t:tt)*) => (crate::log(&format_args!($($t)*).to_string()))
}

// A static reference to our client that keeps it alive for the duration of the program
// thread_local because we don't intend to have any other threads that would access this value
thread_local!(static CLIENT_ANCHOR: LateInit<Rc<RefCell<Client>>> = Default::default());

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
  utils::set_panic_hook();

  let client = Client::new()?;
  CLIENT_ANCHOR.with(move |anchor| {
    anchor.init(client)
  });

  Ok(())
}

#[derive(Debug)]
struct MatchState {
  history: MoveHistory,
    // TODO this is now only used to figure it if we're viewing the last state. Combine this with state in a more ergonomic way
    // Also setting the currently shown index should implicitly update the game_state field
  display_move_idx: usize,
  game_state: Game,
}

#[derive(Debug)]
pub struct Client {
  match_state: Option<MatchState>,
  control: MatchControl,
  display: LateInit<Display>,
  connection: LateInit<Connection>,
}

#[derive(Debug)]
struct Display {
  client_ref: Weak<RefCell<Client>>,
  regenerated_closures: Vec<Closure<dyn Fn()>>,
  permanent_closures: Vec<Closure<dyn Fn(Event)>>,
}

enum HistoryJumpTarget {
  Start,
  End
}

impl Client {
  fn new() -> Result<Rc<RefCell<Self>>, JsValue> {
    let client = Rc::new(RefCell::new(Self {
      match_state: None,
      control: MatchControl::None,
      display: Default::default(),
      connection: Default::default(),
    }));

    let connection = Connection::new(Rc::downgrade(&client))?;
    let display = Display::new(Rc::downgrade(&client))?;
    client.borrow().display.init(display);
    client.borrow().connection.init(connection);

    Ok(client)
  }

  fn on_connected(&self) {
    let window = web_sys::window().expect("Couldn't get window");
    let pathname = window.location().pathname().expect("No pathname available");
    let match_id = {
      let path_suffix = &pathname[1..];
      if path_suffix.len() == 0 {
        None
      } else {
        Some(path_suffix.to_string())
      }
    };

    let msg = ClientMessage::JoinMatch(match_id);
    self.send_message(&msg);
  }

  fn on_message(&mut self, msg: &[u8]) {
    let msg: Result<ServerMessage, _> = serde_cbor::from_slice(msg);
    if let Err(e) = &msg {
      console_log!("Could not deserialize game message {:?}: {:?}", msg, e);
      return;
    }
    let msg = msg.unwrap();
    let mut process_message = |msg| -> Result<(), JsValue> {
      match msg {
        ServerMessage::Control(control) => {
          self.control = control;

          let window = web_sys::window().expect("Couldn't get window");
          let document = window.document().expect("Couldn't get document");
          let player_status: HtmlElement = document.get_element_by_id("player_status").expect("Couldn't find 'player_status' element").dyn_into()?;
          player_status.set_inner_text(&control_message(control));

          // TODO consolidate callsites for this logic.
          // This is here because we might not have had text in the status display before, in which case this changes the available height for the board
          self.adjust_board_width();
        },
        ServerMessage::ActionInvalid(reason) => {
          console_log!("Invalid action was attempted: {}", reason);
        },
        ServerMessage::GameState(match_id_hash, moves, size) => {
          let window = web_sys::window().expect("Couldn't get window");
          let pathname = window.location().pathname().expect("Couldn't get pathname");
          if pathname[1..] != match_id_hash {
            let history = window.history().expect("Couldn't get history");
            history.replace_state_with_url(&JsValue::NULL, "", Some(&match_id_hash)).expect("Couldn't replace history state");
          }
          // window.location().set_pathname(&match_id_hash).expect("Couldn't set pathname to match id hash.");

            // TODO error handling instead of panic?
          let history = MoveHistory::from_moves(moves, size).unwrap_or_else(|e| panic!("Move list sent by server was invalid: Move {}, Reason {}", e.0, e.1));
          let display_move_idx = history.moves().len();
          let game_state = history.last();
          let match_state = MatchState {
            history,
            display_move_idx,
            game_state
          };
          self.match_state = Some(match_state);
          self.display.update(self.match_state.as_ref().unwrap());
        }
      }

      Ok(())
    };

    process_message(msg.clone())
      .unwrap_or_else(|e| console_log!("Error processing the game message {:?}: {:?}", msg, e));
  }

  fn on_click(&mut self, click_loc: Location) {
    fn handle_click(game: &mut Game, click_loc: Location) -> Result<(), ActionInvalidReason> {
      use ActionInvalidReason::*;
      use PlacementInvalidReason::*;
      use MovementInvalidReason::*;

      let board = game.board();

      match game.move_state().clone() {
        MoveState::Start => {
          let target_stack = &board[click_loc];
          if target_stack.count() == 0 {

            let action = MoveAction::Place { loc: click_loc, kind: StoneKind::FlatStone };
            let try_capstone = match game.do_action(action) {
              Ok(_) => Ok(false),
              Err(e) => if let PlacementInvalid(NoStoneAvailable) = e {
                Ok(true)
              } else {
                Err(e)
              }
            }?;

            if try_capstone {
              let action = MoveAction::Place { loc: click_loc, kind: StoneKind::Capstone };
              game.do_action(action)?;
            }
          } else {
            let pickup_count = std::cmp::min(target_stack.count(), board.size().get());
            let action = MoveAction::Pickup { loc: click_loc, count: pickup_count };
            game.do_action(action).or_else(|e| if let MovementInvalid(StartNotControlled) = e { Ok(()) } else { Err(e) })?;
          }
        },
        MoveState::Placed { loc, kind } => {
          if click_loc == loc {
            game.undo();

            let mut new_kind = kind;
            loop {
              new_kind = match new_kind {
                StoneKind::FlatStone => StoneKind::StandingStone,
                StoneKind::StandingStone => StoneKind::Capstone,
                StoneKind::Capstone => StoneKind::FlatStone
              };

              let action = MoveAction::Place { loc, kind: new_kind };
              let success = match game.do_action(action) {
                Ok(_) => Ok(true),
                Err(e) => {
                  if let PlacementInvalid(ref placement_reason) = e {
                    match placement_reason {
                      NoStoneAvailable => Ok(false),
                      StoneKindNotValid => Ok(false),
                      _ => Err(e)
                    }
                  } else {
                    Err(e)
                  }
                }
              }?;

              if success {
                break;
              }
            }
          } else {
            game.undo();
          }
        },
        MoveState::Movement { start, cur_loc, dir, carry, .. } => {
          // Set up a mapping of space -> resulting action on the game
          let mut space_actions: Vec<(Location, Box<dyn FnOnce(&mut Game)->Result<(), ActionInvalidReason>>)> = vec![];

          space_actions.push(
            (cur_loc, Box::new(
              |game| -> _ {
                  // Don't try to do a drop that will fail if we have nothing to drop.
                  // We still allow the user to click on this space even if it will do nothing because we don't want this space to undo their move.
                if carry.count() == 0 { return Ok(()) }

                let action = MoveAction::Drop { count: 1 };
                game.do_action(action)?;

                if let MoveState::Movement { carry, drops, .. } = game.move_state().clone() {
                  // If we've dropped all the stones we picked up without moving, undo the whole action
                  // TODO give the game a "undo to last start" method
                  if carry.count() == 0 && drops.len() == 0 {
                    loop {
                      game.undo();
                      if let MoveState::Start = game.move_state() {
                        break;
                      }
                    }
                  }
                }

                Ok(())
              })
            ),
          );

          if let Some(dir) = dir {
              // Get all locations between start and cur_loc, excluding both start and cur_loc
            let mut prev_locs = vec![];
            let mut loc = start.move_along(dir, game.board().size()).unwrap();
            while loc != cur_loc {
              prev_locs.push(loc);
              loc = loc.move_along(dir, game.board().size()).unwrap();
            }

            space_actions.extend(prev_locs.into_iter().map(|target_loc| {
              (target_loc, Box::new(
                move |game: &mut Game| -> _ {
                  loop {
                    game.undo();
                    if let MoveState::Movement { cur_loc, .. } = game.move_state() {
                      if *cur_loc == target_loc {
                        break;
                      }
                    }
                  }
                  Ok(())
                }) as Box<_>
              )
            }))
          }

          if carry.count() != 0 { // if we're out of stones, we can't do a MoveAndDropOne action
            let move_drop_locs = 
              if let Some(dir) = dir {
                cur_loc.move_along(dir, board.size()).iter().map(|&loc| (loc, dir)).collect()
              } else {
                cur_loc.neighbours_with_direction(board.size())
              };

            space_actions.extend(move_drop_locs.into_iter().map(|(loc, dir)| {
              (loc, Box::new(
                move |game: &mut Game| -> _ {
                  let action = MoveAction::MoveAndDropOne { dir: dir };
                  game.do_action(action).or_else(|e| {
                    match e {
                      DropTooLarge => Ok(()),
                      MovementInvalid(DropNotAllowed) => Ok(()),
                      _ => Err(e)
                    }
                  })
                }) as Box<_>
              )
            }));
          }

          if let Some((_, action)) = space_actions.into_iter().find(|(loc, _)| *loc == click_loc) {
            action(game)?;
          } else {
            // TODO give the game a "undo to last start" method
            loop {
              game.undo();
              if let MoveState::Start = game.move_state() {
                break;
              }
            }
          }
        }
      }

      Ok(())
    };

    let mut did_update = false;
    if let Some(match_state) = &mut self.match_state {
        // TODO represent "viewing the play state" in a more ergonomic way
      if match_state.display_move_idx == match_state.history.moves().len() {
        let game_state = &mut match_state.game_state;
        if self.control.controls(game_state.active_color()) {
          handle_click(game_state, click_loc)
            .unwrap_or_else(|e| console_log!("Client::on_click attempted an invalid action: {}", e));

          did_update = true;
        }
      }
    } else {
      console_log!("Client::on_click called with no game present!");
    };

    if did_update {
      self.display.update(self.match_state.as_ref().unwrap());
    }
  }

  // TODO all the history move idx change functions could be combined into a single one with an enum for set/offset/jump
  fn on_history_click(&mut self, move_idx: usize) {
    if let Some(match_state) = &mut self.match_state {
      match_state.display_move_idx = move_idx;
      match_state.game_state = match_state.history.state(move_idx);
    }

    self.display.update(self.match_state.as_ref().unwrap());
  }

  fn on_history_step(&mut self, offset: i8) {
    if let Some(match_state) = &mut self.match_state {
        // If we're at index 0, we want to stay there (we have no moves yet)
        // TODO this logic should really determine if the button is clickable in the first place
      if match_state.display_move_idx != 0 {
          // Otherwise, offset and clamp between [1, moves count]. 1 is "after the first move" and moves count is "after the last move"
        match_state.display_move_idx = (match_state.display_move_idx as isize + offset as isize).clamp(1, match_state.history.moves().len() as isize) as usize;
        match_state.game_state = match_state.history.state(match_state.display_move_idx);
      }
    }

    self.display.update(self.match_state.as_ref().unwrap());
  }

  fn on_history_jump(&mut self, target: HistoryJumpTarget) {
    if let Some(match_state) = &mut self.match_state {
      if match_state.display_move_idx != 0 {
        match_state.display_move_idx = match target {
          HistoryJumpTarget::Start => 1,
          HistoryJumpTarget::End => match_state.history.moves().len(),
        };
        match_state.game_state = match_state.history.state(match_state.display_move_idx);
      }
    }

    self.display.update(self.match_state.as_ref().unwrap());
  }

  fn submit_move(&mut self) {
    if let Some(match_state) = &mut self.match_state {
        // TODO represent "viewing the play state" in a more ergonomic way
      if match_state.display_move_idx == match_state.history.moves().len() {
        if let Some(m) = match_state.game_state.finalise_move() {
          self.send_message(&ClientMessage::Move(m)); // TODO copied from send_move, consolidate
        }
      }
    }
    // TODO if move gets rejected from server we need to revert what we did
  }

  fn submit_action(&self, msg: &str) {
    if msg.starts_with("ctrl: ") {
      let ctrl_text = &msg[6..];
      if ctrl_text.starts_with("new ") {
        let size = ctrl_text[4..5].parse::<usize>();
        if let Ok(size) = size {
          let board_size = BoardSize::new(size);
          if let Some(board_size) = board_size {
            self.send_message(&ClientMessage::ResetGame(board_size));
          } else {
            // TODO tell display that the size was invalid
          }
        }
      } else if ctrl_text == "undo" {
        self.send_message(&ClientMessage::UndoMove);
      } else if ctrl_text == "swap" {
        self.send_message(&ClientMessage::SwapPlayers);
      }
    }
  }

  fn send_message(&self, msg: &ClientMessage) {
    let msg_binary_res = serde_cbor::ser::to_vec_packed(msg);
    if let Err(e) = msg_binary_res {
      console_log!("Failed to serialise {:?}: {}", msg, e);
      return;
    }

    self.connection.send_message(&msg_binary_res.unwrap());
  }

  fn adjust_board_width(&self) {
    if let Some(match_state) = self.match_state.as_ref() {
      Display::adjust_board_width(match_state.game_state.board().size())
    }
  }
}

fn control_message(control: MatchControl) -> String {
  let control_str = match control {
    MatchControl::Both => "Both colors".to_string(),
    MatchControl::Single(color) => color.to_string(),
    MatchControl::None => "No color".to_string(),
  };
  format!("You control: {}", control_str)
}

fn clear_children(el: &Element) -> Result<(), wasm_bindgen::JsValue> {
  while let Some(child) = el.last_child() {
    el.remove_child(&child).map(|_| ())?;
  }

  Ok(())
}

impl Display {
  fn new(client_ref: Weak<RefCell<Client>>) -> Result<Self, JsValue> {
    let mut out = Self {
      client_ref: client_ref.clone(),
      regenerated_closures: vec![],
      permanent_closures: vec![],
    };

    let window = web_sys::window().expect("Couldn't get window");
    let document = window.document().expect("Couldn't get document");

    { // Move submit button callback
      let callback = {
        let client_ref = client_ref.clone();

        Closure::wrap(Box::new(move |_event: Event| {
          client_ref.upgrade().unwrap().borrow_mut().submit_move();
        }) as Box<dyn Fn(Event)>)
      };

      let event_target: EventTarget = document.get_element_by_id("move-submit").expect("Couldn't get move-submit div").dyn_into()?;
      event_target.add_event_listener_with_callback("click", callback.as_ref().unchecked_ref())?;
      out.permanent_closures.push(callback);
    }

    { // Action submit key callback
      let input_box: HtmlInputElement = document.get_element_by_id("input").expect("Couldn't get input text box").dyn_into()?;
      let callback = {
        let client_ref = client_ref.clone();
        let input_box = input_box.clone();

        Closure::wrap(Box::new(move |event: Event| {
          let event: KeyboardEvent = event.dyn_into().unwrap();

          // Enter
          if event.key_code() == 13 {
            event.prevent_default();
            client_ref.upgrade().unwrap().borrow_mut().submit_action(&input_box.value());
            input_box.set_value("");
          }
        }) as Box<dyn Fn(Event)>)
      };

      let event_target: EventTarget = input_box.dyn_into()?;
      event_target.add_event_listener_with_callback("keyup", callback.as_ref().unchecked_ref())?;
      out.permanent_closures.push(callback);
    }

    { // History navigation key callback
      let callback = {
        let client_ref = client_ref.clone();

        Closure::wrap(Box::new(move |event: Event| {
          let event: KeyboardEvent = event.dyn_into().unwrap();

          // left arrow, A
          if [37, 65].contains(&event.key_code()) {
            if event.shift_key() {
              client_ref.upgrade().unwrap().borrow_mut().on_history_jump(HistoryJumpTarget::Start)
            } else {
              client_ref.upgrade().unwrap().borrow_mut().on_history_step(-1);
            }
          }
          // right arrow, D
          else if [39, 68].contains(&event.key_code()) {
            if event.shift_key() {
              client_ref.upgrade().unwrap().borrow_mut().on_history_jump(HistoryJumpTarget::End)
            } else {
              client_ref.upgrade().unwrap().borrow_mut().on_history_step( 1);
            }
          }
        }) as Box<dyn Fn(Event)>)
      };

      let event_target: &EventTarget = document.dyn_ref().unwrap();
      event_target.add_event_listener_with_callback("keyup", callback.as_ref().unchecked_ref())?;
      out.permanent_closures.push(callback);
    }
    
    { // Window resize callback
      let callback = {
        let client_ref = client_ref.clone();

        Closure::wrap(Box::new(move |_event: Event| {
          client_ref.upgrade().unwrap().borrow_mut().adjust_board_width();
        }) as Box<dyn Fn(Event)>)
      };

      let event_target: &EventTarget = window.dyn_ref().unwrap();
      event_target.add_event_listener_with_callback("resize", callback.as_ref().unchecked_ref())?;
      out.permanent_closures.push(callback);
    }

    Ok(out)
  }

  fn update(&mut self, match_state: &MatchState) {
      // TODO be consistent about error handling in here - some stuff is `expect`ed, some is handled with the ? operator
    (|| -> Result<(), JsValue> {
      let game = &match_state.game_state;
      let board_size = game.board().size().get();

      let window = web_sys::window().expect("Couldn't get window");
      let document = window.document().expect("Couldn't get document");

        // this stores all on-click closures for the whole display
        // will get re-populated by several parts of display building
      self.regenerated_closures.clear();

      // set board wrapper classes
      let board_wrapper = document.get_element_by_id("board-wrapper").expect("Couldn't get board-wrapper div");
      board_wrapper.set_class_name("");
      board_wrapper.class_list().add_2("board-wrapper", &format!("size-{}", board_size))?;

      // re-populate spaces divs
      let spaces = document.get_element_by_id("spaces").expect("Couldn't get spaces div");
      clear_children(&spaces)?;

      const HEADINGS: [&str; 4] = ["north", "east", "south", "west"];
      let mut space_els = vec![];
      for row in (0..board_size).rev() {
        for col in 0..board_size {
          let color_class = if (col + (row % 2)) % 2 == 0 {
            "dark"
          } else {
            "light"
          };

          let space: HtmlElement = document.create_element("div")?.dyn_into()?;
          space.class_list().add_2("space", color_class)?;

          for heading in &HEADINGS {
            let bridge: HtmlElement = document.create_element("div")?.dyn_into()?;
            bridge.class_list().add_2("bridge", heading)?;
            space.append_child(&bridge)?;
          }

            // TODO it's unnecessary to re-create a callback each time we get new board state
            // Profile if this creates substantial memory churn, and if so change it to cache them and only re-create
            // if the board size changes. (Having a specific phase for that would allow us to get rid of various repeated work anyway)
          let client_ref = self.client_ref.clone();
          let callback = Closure::wrap(Box::new(move || { // TODO prevent default on e: MouseEvent (from web-sys)
            client_ref.upgrade().unwrap().borrow_mut().on_click(Location::from_coords(col, row).unwrap());
          }) as Box<dyn Fn()>);

          space.set_onclick(Some(callback.as_ref().unchecked_ref())); // as_ref().unchecked_ref() gets &Function from Closure
          self.regenerated_closures.push(callback);
          spaces.append_child(&space)?;

          space_els.push((space, Location::from_coords(col, row).unwrap()));
        }
      }

        // add control and bridge classes to space elements for showing bridges
      for (space, loc) in space_els {
        if let Some(ctrl_color) = game.board().road_control_at(loc) {
          let ctrl_class = match ctrl_color {
            Color::White => "ctrl-light",
            Color::Black => "ctrl-dark",
          };
          space.class_list().add_1(ctrl_class)?;

          let dir_to_heading = |dir| {
            use Direction::*;
            HEADINGS[match dir { Up => 0, Right => 1, Down => 2, Left => 3 }]
          };

          let neighbours_with_heading = loc.neighbours_with_direction(game.board().size())
            .into_iter().map(|(loc, dir)| (loc, dir_to_heading(dir)));

            // add connections between neighbours
          for (neighbour_loc, heading) in neighbours_with_heading {
            if let Some(neighbour_ctrl_color) = game.board().road_control_at(neighbour_loc) {
              if neighbour_ctrl_color == ctrl_color {
                space.class_list().add_1(heading)?;
              }
            }
          }

            // add connections to the edge of the board
          for touched_heading in loc.touching_sides(game.board().size()).dirs().into_iter().map(dir_to_heading) {
            space.class_list().add_1(touched_heading)?;
          }
        }
      }

      // re-populate ranks
      let ranks = document.get_element_by_id("ranks").expect("Couldn't get ranks div");
      clear_children(&ranks)?;
      for rank in (1..=board_size).rev() {
        let rank_el: HtmlElement = document.create_element("div")?.dyn_into()?;
        rank_el.set_inner_text(&rank.to_string());
        ranks.append_child(&rank_el)?;
      }

      // re-populate files
      let files = document.get_element_by_id("files").expect("Couldn't get files div");
      clear_children(&files)?;
      for file in 0..board_size {
        let file_el: HtmlElement = document.create_element("div")?.dyn_into()?;
        file_el.set_inner_text(&file_idx_to_char(file).unwrap().to_string());
        files.append_child(&file_el)?;
      }

      // set up stones
      let stones = document.get_element_by_id("stones").expect("Couldn't get stones div");
      clear_children(&stones)?;

      let make_stone_element = |stone: &Stone, x, y, idx: usize, buried_count| -> Result<HtmlElement, JsValue> {
        let wrapper: HtmlElement = document.create_element("div")?.dyn_into()?;
        wrapper.class_list().add_1("stone-wrapper")?;

        let buried = idx < buried_count;
          // Draw the buried stones at their normal indices
          // Anything above that gets offset down by the buried count, i.e. the first non-buried stone is drawn at index 0
        let stack_draw_idx = if buried {
          idx
        } else {
          idx - buried_count
        };

        let z = 
          if stone.kind == StoneKind::FlatStone {
            stack_draw_idx
          } else {
            // standing and cap stones should be centered on the stone below them, 
            // so draw them at z-1 so they get the same vertical offset as the stone below them
            stack_draw_idx.saturating_sub(1) // don't go below 0
          };

        let transform_x = x * 100;
        let transform_y = (y as isize) * -100 + (z as isize) * -7;
        wrapper.style().set_property("transform", &format!("translate({}%, {}%)", transform_x, transform_y))?; // TODO add logical z transform to get correct depth sorting

        let color_class = match stone.color {
          Color::White => "light",
          Color::Black => "dark"
        };

        let kind_class = match stone.kind {
          StoneKind::FlatStone => None,
          StoneKind::StandingStone => Some("standing"),
          StoneKind::Capstone => Some("cap")
        };

        let stone_el = document.create_element("div")?;
        stone_el.class_list().add_2("stone", color_class)?;

        if let Some(kind_class) = kind_class {
          stone_el.class_list().add_1(kind_class)?;
        }

        if buried {
          stone_el.class_list().add_1("buried")?;
        }

        wrapper.append_child(&stone_el)?;
        Ok(wrapper)
      };

        // Draw a stack, with the first stone placed at logical z base_draw_z
      let make_stack_elements = |base_stack: &StoneStack, hover_stack: Option<&StoneStack>, x, y| -> Result<(), JsValue> {
        let buried_count = (base_stack.count() + hover_stack.map_or(0, |stack| stack.count())).saturating_sub(board_size);

        for (idx, stone) in base_stack.iter().enumerate() {
          let wrapper_el = make_stone_element(stone, x, y, idx, buried_count)?;
          stones.append_child(&wrapper_el)?;
        }

        if let Some(hover_stack) = hover_stack {
          let highest_base_idx = base_stack.count() - buried_count;
          for (idx, stone) in hover_stack.iter().enumerate() {
            let wrapper_el = make_stone_element(stone, x, y, highest_base_idx + 2 + idx, 0)?;
            stones.append_child(&wrapper_el)?;
          }
        }

        Ok(())
      };

      for x in 0..board_size {
        for y in 0..board_size {
          let stack = game.board().get(x, y);
          let hover_stack = 
            if let MoveState::Movement { cur_loc, carry, .. } = game.move_state() {
              if cur_loc == &(x, y) { Some(carry) } else { None }
            } else {
              None
            };

          make_stack_elements(stack, hover_stack, x, y)?;
        }
      }

      let (mut white_control, mut black_control) = game.board().count_flats_control();

      // TODO clean up all the control modification logic

        // Adjust the control counts to take the carried stack into account
        // Adds the control of the carried stack if applicable
        // Removes the control of the stack being hovered (if it has stones)
        // With this logic, the count shows what the counts are if the carried stack was placed down at the hovered location
      if let MoveState::Movement { cur_loc, carry, .. } = game.move_state() {
        let existing_stack = &game.board()[*cur_loc];
        if let Some(stone) = carry.top_stone() {
          if let Some(stone) = existing_stack.top_stone() {
            if let StoneKind::FlatStone = stone.kind {
              match stone.color {
                Color::White => white_control -= 1,
                Color::Black => black_control -= 1,
              }
            }
          }

          if let StoneKind::FlatStone = stone.kind {
            match stone.color {
              Color::White => white_control += 1,
              Color::Black => black_control += 1,
            }
          }
        }
      }

      let control_count_light = document.get_element_by_id("control-count-light").expect("Couldn't get control-count-light div");
      let control_count_light: HtmlElement = control_count_light.dyn_into()?;
      control_count_light.set_inner_text(&format!("{}", white_control));

      let control_count_dark = document.get_element_by_id("control-count-dark").expect("Couldn't get control-count-dark div");
      let control_count_dark: HtmlElement = control_count_dark.dyn_into()?;
      control_count_dark.set_inner_text(&format!("{}", black_control));

      let total_control = white_control + black_control;
      let (white_control_fraction, black_control_fraction) = 
        if total_control > 0 {
          let white_control_fraction = (white_control as f32) / (total_control as f32);
          (white_control_fraction, 1.0 - white_control_fraction)
        } else {
          (0.5, 0.5)
        };

      let white_height_percent = 10.0 + white_control_fraction * 80.0;
      let black_height_percent = 10.0 + black_control_fraction * 80.0;

      let control_bar_light = document.get_element_by_id("control-bar-light").expect("Couldn't get control-bar-light div");
      let control_bar_light: HtmlElement = control_bar_light.dyn_into()?;
      control_bar_light.style().set_property("height", &format!("{}%", white_height_percent))?;

      let control_bar_dark = document.get_element_by_id("control-bar-dark").expect("Couldn't get control-bar-dark div");
      let control_bar_dark: HtmlElement = control_bar_dark.dyn_into()?;
      control_bar_dark.style().set_property("height", &format!("{}%", black_height_percent))?;

      let history = document.get_element_by_id("history").expect("Couldn't get history div");

        // Determine if we are scrolled to the bottom of the moves list before we clear & reconstruct it
      let scroll_to_restore = if history.scroll_top() < (history.scroll_height() - history.client_height()) {
        Some(history.scroll_top())
      } else {
        None
      };

      clear_children(&history)?;

      let moves = match_state.history.moves();
      for move_idx in 0..moves.len() {
        if move_idx % 2 == 0 {
          let turn_number: HtmlElement = document.create_element("span")?.dyn_into()?;
          turn_number.class_list().add_1("turn-number")?;
          turn_number.set_inner_text(&format!("{}.", move_idx / 2 + 1));
          history.append_child(&turn_number)?;  
        }

        let move_text: HtmlElement = document.create_element("span")?.dyn_into()?;
        move_text.class_list().add_1("move-text")?;
        if let Some(m) = moves.get(move_idx) {
          move_text.set_inner_text(&m.to_string());
        }

        if move_idx == match_state.display_move_idx - 1 { // -1 since move_idx is 0-indexed, but display_move_idx is 1 indexed
          move_text.class_list().add_1("active")?;
        }

        let client_ref = self.client_ref.clone();
        let callback = Closure::wrap(Box::new(move || { // TODO prevent default on e: MouseEvent (from web-sys)
          client_ref.upgrade().unwrap().borrow_mut().on_history_click(move_idx + 1); // +1 because here we numbered them from 0, but generally the first move has index 1
        }) as Box<dyn Fn()>);

        move_text.set_onclick(Some(callback.as_ref().unchecked_ref())); // as_ref().unchecked_ref() gets &Function from Closure
        self.regenerated_closures.push(callback);

        history.append_child(&move_text)?;
      }

      if let Some(scroll_top) = scroll_to_restore {
        history.set_scroll_top(scroll_top)
      } else {
        history.set_scroll_top(history.scroll_height() - history.client_height());
      }

        // TODO The callbacks for these elements shouldn't be re-gen'd every display update. 
      let history_back: HtmlElement    = document.get_element_by_id("history-back").expect("Couldn't get history-back div").dyn_into()?;
      let history_forward: HtmlElement = document.get_element_by_id("history-forward").expect("Couldn't get history-forward div").dyn_into()?;

      if moves.len() == 0 || match_state.display_move_idx == 1 {
        history_back.class_list().add_1("disabled")?;
      } else {
        history_back.class_list().remove_1("disabled")?;
      }

      if moves.len() == 0 || match_state.display_move_idx == moves.len() {
        history_forward.class_list().add_1("disabled")?;
      } else {
        history_forward.class_list().remove_1("disabled")?;
      }

        // TODO this can become a simple array once IntoIterator is implemented properly for arrays, which allows us to get an owning iterator in the for loop.
        // See this PR: https://github.com/rust-lang/rust/pull/65819
      let nav_els = [(history_back, -1), (history_forward, 1)];
      for (el, offset) in nav_els.iter() {
        let client_ref = self.client_ref.clone();
        let offset = *offset; // copy the offset value so we don't reference nav_els from inside our move closure

        let callback = Closure::wrap(Box::new(move || {
          client_ref.upgrade().unwrap().borrow_mut().on_history_step(offset);
        }) as Box<dyn Fn()>);

        el.set_onclick(Some(callback.as_ref().unchecked_ref()));
        self.regenerated_closures.push(callback);
      }

      let get_status_text = || -> Result<String, std::fmt::Error> {
        let mut status_text = String::new();
        match game.state() {
          GameState::Ongoing => { write!(&mut status_text, "Turn {}, {}'s go.", game.turn(), game.active_color())?; },
          GameState::Draw => { write!(&mut status_text, "Draw.")?; },
          GameState::Win(color, kind) => {
            write!(&mut status_text, "{} wins ", color)?;
            match kind {
              WinKind::Road => write!(&mut status_text, "by road."),
              WinKind::BoardFilled => write!(&mut status_text, "by flats (board full)."),
              WinKind::PlayedAllStones(color) => write!(&mut status_text, "by flats ({} played all stones).", color)
            }?;
          }
        }
        Ok(status_text)
      };

      let move_submit: HtmlElement = document.get_element_by_id("move-submit").expect("Couldn't get move-submit button").dyn_into()?;
      if game.move_state() == &MoveState::Start {
        move_submit.class_list().add_1("disabled")?;
      } else {
        move_submit.class_list().remove_1("disabled")?;
      }

      let status_text = get_status_text().expect("Couldn't write status text");

      let game_status = document.get_element_by_id("game_status").expect("Couldn't get game_status div");
      let game_status: HtmlElement = game_status.dyn_into()?;
      game_status.set_inner_text(&status_text);

      let stone_counter_light_flat = document.get_element_by_id("stone-counter-light-flat").expect("Couldn't get stone-counter-light-flat div");
      let stone_counter_light_flat: HtmlElement = stone_counter_light_flat.dyn_into()?;
      stone_counter_light_flat.set_inner_text(&(game.held_stones().get(Color::White).flat().to_string()));

      let stone_counter_light_cap = document.get_element_by_id("stone-counter-light-cap").expect("Couldn't get stone-counter-light-cap div");
      let stone_counter_light_cap: HtmlElement = stone_counter_light_cap.dyn_into()?;
      stone_counter_light_cap.set_inner_text(&(game.held_stones().get(Color::White).capstone().to_string()));

      let stone_counter_dark_flat = document.get_element_by_id("stone-counter-dark-flat").expect("Couldn't get stone-counter-dark-flat div");
      let stone_counter_dark_flat: HtmlElement = stone_counter_dark_flat.dyn_into()?;
      stone_counter_dark_flat.set_inner_text(&(game.held_stones().get(Color::Black).flat().to_string()));

      let stone_counter_dark_cap = document.get_element_by_id("stone-counter-dark-cap").expect("Couldn't get stone-counter-dark-cap div");
      let stone_counter_dark_cap: HtmlElement = stone_counter_dark_cap.dyn_into()?;
      stone_counter_dark_cap.set_inner_text(&(game.held_stones().get(Color::Black).capstone().to_string()));

      Display::adjust_board_width(game.board().size());

      Ok(())
    })()
      .unwrap_or_else(|e| console_log!("Display::update error: {:?}", e));
  }

  fn adjust_board_width(board_size: BoardSize) {
    (|| -> Result<(), JsValue> {
      let board_size = board_size.get() as f64;

      let window = web_sys::window().expect("Couldn't get window");
      let document = window.document().expect("Couldn't get document");

      let main_wrapper = document.get_element_by_id("main-wrapper").expect("Couldn't get main-wrapper div");
      let header = document.get_element_by_id("header").expect("Couldn't get header");

      let board_wrapper: HtmlElement = document.get_element_by_id("board-wrapper").expect("Couldn't get board-wrapper div").dyn_into()?;
      let board_el: HtmlElement = document.get_element_by_id("board").expect("Couldn't get board div").dyn_into()?;

      let history_wrapper: HtmlElement = document.get_element_by_id("history-wrapper").expect("Couldn't get history-wrapper div").dyn_into()?;
      let files = document.get_element_by_id("files").expect("Couldn't get files div");
      let ranks = document.get_element_by_id("ranks").expect("Couldn't get ranks div");
      let control_display = document.get_element_by_id("control-display").expect("Couldn't get control-display div");

      let move_submit = document.get_element_by_id("move-submit").expect("Couldn't get move-submit button");
      let status_display = document.get_element_by_id("status-display").expect("Couldn't get status-display div");
      let input_row = document.get_element_by_id("input-row").expect("Couldn't get input-row div");
      let footer = document.get_element_by_id("footer").expect("Couldn't get footer");

      let main_wrapper_size = get_dimensions(&window, &main_wrapper)?;
      let header_size = get_dimensions(&window, &header)?;
      let history_wrapper_size = get_dimensions(&window, &history_wrapper)?;
      let files_size = get_dimensions(&window, &files)?;
      let ranks_size = get_dimensions(&window, &ranks)?;
      let move_submit_size = get_dimensions(&window, &move_submit)?;
      let control_display_size = get_dimensions(&window, &control_display)?;
      let status_display_size = get_dimensions(&window, &status_display)?;
      let input_row_size = get_dimensions(&window, &input_row)?;
      let footer_size = get_dimensions(&window, &footer)?;

      // TODO I'm basically doing layouting myself at this point. This feels extremely not in the spirit of css
      let available_height = main_wrapper_size.1 - header_size.1 - files_size.1 - move_submit_size.1 - status_display_size.1 - input_row_size.1 - footer_size.1;
        // take the maximum of total width of everything to the left and right of the board
        // then pretend we have that width on either side.
        // this is necessary so that after we center the wrapper to the board neither side overflows outside of the window.
      let available_width = main_wrapper_size.0 - (history_wrapper_size.0 + ranks_size.0).max(control_display_size.0) * 2.0;

      let square_size_from_height = available_height / (board_size + 1.0); // The +1 here is to take the stone counter row at the bottom into account
      let square_size_from_width = available_width / board_size;

      let square_size = square_size_from_height.min(square_size_from_width);
      let mut board_size = square_size * board_size;

      let set_board_size = |size: f64| -> std::result::Result<(), JsValue> {
        board_el.style().set_property("width", &(size.to_string()))?;
        board_el.style().set_property("height", &(size.to_string()))?;

        history_wrapper.style().set_property("height", &(size.to_string()))?;

        Ok(())
      };

      // Set the board element's size based on our calculations
      set_board_size(board_size)?;

      // The ranks element generally shrinks/expands to match the size of the board element that we set.
      // However if the board element is smaller than the ranks, ranks determines the minimum height of the row.
      // After we set the board size, we measure the ranks height again to see if we went below the minimum set by it.
      // If we did, set our board size to match the ranks height.
      let ranks_size = get_dimensions(&window, &ranks)?;
      if ranks_size.1 > board_size {
        board_size = ranks_size.1;
        set_board_size(board_size)?;
      }

        // center the board display on the actual grid of spaces
      let left_offset = main_wrapper_size.0 / 2.0 - board_size / 2.0 - ranks_size.0 - history_wrapper_size.0;
      board_wrapper.style().set_property("margin-left", &(left_offset.to_string()))?;

      Ok(())
    })()
      .unwrap_or_else(|e| console_log!("Display::adjust_board_width error: {:?}", e));
  }
}

// Based on this code: https://github.com/hughsk/element-size/blob/master/index.js
fn get_dimensions(window: &Window, element: &Element) -> Result<(f64, f64), JsValue> {
  let client_rect = element.get_bounding_client_rect();
  let mut width = client_rect.width();
  let mut height = client_rect.height();

  let computed_style = window.get_computed_style(element)?;
  if let Some(computed_style) = computed_style {
      // TODO no idea if these values always have px at the end. If not this will panic during the parse -> expect
    width += 
      computed_style.get_property_value("margin-left")?.trim_end_matches("px").parse::<f64>().unwrap_or(0.0) +
      computed_style.get_property_value("margin-right")?.trim_end_matches("px").parse::<f64>().unwrap_or(0.0);

    height += 
      computed_style.get_property_value("margin-top")?.trim_end_matches("px").parse::<f64>().unwrap_or(0.0) +
      computed_style.get_property_value("margin-bottom")?.trim_end_matches("px").parse::<f64>().unwrap_or(0.0);
  }

  Ok((width, height))
}
