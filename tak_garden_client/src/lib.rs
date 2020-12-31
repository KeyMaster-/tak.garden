mod utils;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement};
use tak_garden_common::{ServerMessage, ClientMessage};
use rustak::{
  Game, GameState, MoveState, MoveAction, WinKind,
  BoardSize, Location, Color, 
  StoneKind, StoneStack,
  ActionInvalidReason, MovementInvalidReason,
  file_idx_to_char,
};
use std::fmt::Write;
use std::cell::RefCell;
use std::rc::Rc;

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

macro_rules! console_log {
  ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}


#[wasm_bindgen(module = "/js/connection.js")]
extern "C" {
  pub type Connection;

  #[wasm_bindgen(method)]
  fn send_message(this: &Connection, msg: &[u8]);
}

#[wasm_bindgen(start)]
pub fn start() {
  utils::set_panic_hook();
}

#[wasm_bindgen]
pub fn init(connection: Connection) -> ClientInterface {
  let client = Client::new(connection);
  let client_ref = Rc::new(RefCell::new(client));
  let display = Display::new(client_ref.clone());
  client_ref.borrow_mut().set_display(display);

  ClientInterface::new(client_ref)
}

#[wasm_bindgen]
pub struct ClientInterface(Rc<RefCell<Client>>);

impl ClientInterface {
  fn new(client_ref: Rc<RefCell<Client>>) -> Self {
    Self(client_ref)
  }
}

#[wasm_bindgen]
impl ClientInterface {
  pub fn on_message(&self, msg: &[u8]) {
    self.0.borrow_mut().on_message(msg);
  }

  pub fn submit_action(&self, msg: &str) {
    self.0.borrow().submit_action(msg);
  }

    // this is to submit a partial move
  pub fn submit_move(&self) {
    self.0.borrow_mut().submit_move();
  }

    // TODO this should just go to the view directly
  pub fn adjust_board_width(&self) {
    self.0.borrow().adjust_board_width()
  }
}

struct Client {
  connection: Connection,
  game: Option<Game>,
  display: Option<Display>, // Is only None before Client is fully initialised. TODO replace with LateInit from once_cell (given in docs)
}

struct Display {
  client_ref: Rc<RefCell<Client>>,
  click_closures: Vec<Closure<dyn Fn()>>,
}

impl Client {
  fn new(connection: Connection) -> Self {
    Self {
      connection,
      game: None,
      display: None
    }
  }

  fn set_display(&mut self, display: Display) {
    self.display = Some(display);
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
        ServerMessage::Control(color_opt) => {
          let window = web_sys::window().expect("Couldn't get window");
          let document = window.document().expect("Couldn't get document");
          let player_status: HtmlElement = document.get_element_by_id("player_status").expect("Couldn't find 'player_status' element").dyn_into()?;
          player_status.set_inner_text(&control_message(color_opt));

          // TODO consolidate callsites for this logic.
          // This is here because we might not have had text in the output before, in which case this changes the available height for the board
          self.adjust_board_width();
        },
        ServerMessage::ActionInvalid(reason) => {
          let window = web_sys::window().expect("Couldn't get window");
          let document = window.document().expect("Couldn't get document");
          let output: HtmlElement = document.get_element_by_id("output").expect("Couldn't find 'output' element").dyn_into()?;
          output.set_inner_text(&reason);

          // TODO consolidate callsites for this logic. See above.
          self.adjust_board_width();
        },
        ServerMessage::GameState(game) => {
          self.game = Some(game);
          self.display.as_mut().unwrap().update(self.game.as_ref().unwrap());
        }
      }

      Ok(())
    };

    process_message(msg.clone())
      .unwrap_or_else(|e| console_log!("Error processing the game message {:?}: {:?}", msg, e));
  }

  fn on_click(&mut self, click_loc: Location) {
    let handle_click = |game: &mut Game| -> Result<(), ActionInvalidReason> {
      use ActionInvalidReason::*;
      use MovementInvalidReason::*;

      let board = game.board();

      // TODO check that it's your turn
      match game.move_state() {
        MoveState::Start => {
          let target_stack = &board[click_loc];
          if target_stack.count() == 0 {
            // TODO pick capstone if no flat stones left. Needs knowledge of what color we're playing
            let action = MoveAction::Place { loc: click_loc, kind: StoneKind::FlatStone };
            game.do_action(action)?; // TODO error handling
          } else {
            let pickup_count = std::cmp::min(target_stack.count(), board.size().get());
            let action = MoveAction::Pickup { loc: click_loc, count: pickup_count };
            game.do_action(action).or_else(|e| if let MovementInvalid(StartNotControlled) = e { Ok(()) } else { Err(e) })?;
          }
        },
        MoveState::Placed { loc, kind } => {
          if click_loc == *loc {
            // get placed stone type
            // undo placement
            // cycle stone type (flat -> standing -> cap -> flat)
            // check if new stone type is available
            // re-do placement with cycled stone type if so
          } else {
            // undo placement
          }
        },
        MoveState::Movement { cur_loc, dir, .. } => {
          if click_loc == *cur_loc {
            let action = MoveAction::Drop { count: 1 };
            game.do_action(action).or_else(|e| if let DropTooLarge = e { Ok(()) } else { Err(e) })?;
          } else {
            let valid_locs = if let Some(dir) = dir {
              cur_loc.move_along(*dir, board.size()).iter().map(|&loc| (loc, *dir)).collect()
            } else {
              cur_loc.neighbours_with_direction(board.size())
            };

            if let Some((_, dir)) = valid_locs.iter().find(|(loc, _)| *loc == click_loc) {
              let action = MoveAction::MoveAndDropOne { dir: *dir };
              game.do_action(action).or_else(|e| {
                match e {
                  DropTooLarge => Ok(()),
                  MovementInvalid(DropNotAllowed) => Ok(()),
                  _ => Err(e)
                }
              })?;
            }
          }
        }
      }

      Ok(())
    };

    if let Some(game) = &mut self.game {
      if let Err(e) = handle_click(game) {
        console_log!("Client::on_click attempted an invalid action: {}", e);
      }
    } else {
      console_log!("Client::on_click called with no game present!");
    }

    self.display.as_mut().unwrap().update(self.game.as_ref().unwrap());
  }

  fn submit_move(&mut self) {
    if let Some(m) = self.game.as_ref().unwrap().move_state().clone().to_move() {
      self.send_message(&ClientMessage::Move(m)); // TODO copied from send_move, consolidate
    }
    // check if partial move is a completed move
    // convert move state into full move
    // send move to server
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
            // send_text(&tx_2, &format!("msg: Game size {} is not a valid game size.", size));

            // send_game_msg(&tx_2, &ServerMessage::ActionInvalid(format!("Game size {} is not a valid game size.", size)));
          }
        }
      }
    } else {
      self.send_move(msg);
    }
  }

  fn send_move(&self, msg: &str) {
    let move_parse_res = msg.parse();
    if let Ok(m) = move_parse_res {
      self.send_message(&ClientMessage::Move(m));
    } else {
      //TODO tell display that the submitted move is wrong
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
    if let Some(game) = self.game.as_ref() {
      Display::adjust_board_width(game.board().size())
    }
  }
}

fn control_message(c: Option<Color>) -> String {
  let color_str = match c {
    Some(color) => color.to_string(),
    None => "No color".to_string()
  };
  format!("You control: {}", color_str)
}

fn clear_children(el: &Element) -> Result<(), wasm_bindgen::JsValue> {
  while let Some(child) = el.last_child() {
    el.remove_child(&child).map(|_| ())?;
  }

  Ok(())
}

impl Display {
  fn new(client_ref: Rc<RefCell<Client>>) -> Self {
    Self {
      client_ref,
      click_closures: vec![]
    }
  }

  fn update(&mut self, game: &Game) {
      // TODO be consistent about error handling in here - some stuff is `expect`ed, some is handled with the ? operator
    (|| -> Result<(), JsValue> {
      let board_size = game.board().size().get();

      let window = web_sys::window().expect("Couldn't get window");
      let document = window.document().expect("Couldn't get document");

      // set board wrapper classes
      let board_wrapper = document.get_element_by_id("board-wrapper").expect("Couldn't get board-wrapper div");
      board_wrapper.set_class_name("");
      board_wrapper.class_list().add_2("board-wrapper", &format!("size-{}", board_size))?;

      // re-populate spaces divs
      let spaces = document.get_element_by_id("spaces").expect("Couldn't get spaces div");
      clear_children(&spaces)?;
      self.click_closures.clear();

      for row in 0..board_size {
        for col in 0..board_size {
          let color_class = if (col + (row % 2)) % 2 == 0 {
            "dark"
          } else {
            "light"
          };

          let space: HtmlElement = document.create_element("div")?.dyn_into()?;
          space.class_list().add_2("space", color_class)?;

            // TODO it's unnecessary to re-create a callback each time we get new board state
            // Profile if this creates substantial memory churn, and if so change it to cache them and only re-create
            // if the board size changes. (Having a specific phase for that would allow us to get rid of various repeated work anyway)
          let client_ref = self.client_ref.clone();
          let callback = Closure::wrap(Box::new(move || { // TODO prevent default on e: MouseEvent (from web-sys)
            client_ref.borrow_mut().on_click(Location::from_coords(col, (board_size - 1) - row).unwrap());
          }) as Box<dyn Fn()>);

          space.set_onclick(Some(callback.as_ref().unchecked_ref())); // as_ref().unchecked_ref() converts gets &Function from Closure
          self.click_closures.push(callback);
          spaces.append_child(&space)?;
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

      let stone_draw_z = |logical_z, kind| -> usize {
        if kind == StoneKind::FlatStone {
          logical_z
        } else {
          // standing and cap stones should be centered on the stone below them, 
          // so draw them at z-1 so they get the same vertical offset as the stone below them
          logical_z.saturating_sub(1) // don't go below 0
        }
      };

      let make_stack_elements = |stack: &StoneStack, x, y, base_z| -> Result<(), JsValue> {
        for (idx, stone) in stack.iter().enumerate() {
          let color_class = match stone.color {
            Color::White => "light",
            Color::Black => "dark"
          };

          let kind_class = match stone.kind {
            StoneKind::FlatStone => None,
            StoneKind::StandingStone => Some("standing"),
            StoneKind::Capstone => Some("cap")
          };

          let z = stone_draw_z(base_z + idx, stone.kind);

          let wrapper: HtmlElement = document.create_element("div")?.dyn_into()?;
          wrapper.class_list().add_1("stone-wrapper")?;

          let transform_x = x * 100;
          let transform_y = (y as isize) * -100 + (z as isize) * -7;
          wrapper.style().set_property("transform", &format!("translate({}%, {}%)", transform_x, transform_y))?; // TODO add logical z transform to get correct depth sorting

          let stone_el = document.create_element("div")?;
          stone_el.class_list().add_2("stone", color_class)?;

          if let Some(kind_class) = kind_class {
            stone_el.class_list().add_1(kind_class)?;
          }

          wrapper.append_child(&stone_el)?;
          stones.append_child(&wrapper)?;
        }

        Ok(())
      };

      for x in 0..board_size {
        for y in 0..board_size {
          let stack = game.board().get(x, y);
          make_stack_elements(stack, x, y, 0)?;
        }
      }

      if let MoveState::Movement { cur_loc, carry, .. } = game.move_state() {
        let existing_stack = &game.board()[*cur_loc];
        let base_z = 
          if let Some(stone) = existing_stack.top_stone() {
            stone_draw_z(existing_stack.count() - 1, stone.kind) + 2 // +2 to leave 1 stone's worth of space between what's there and what's held
          } else {
            1 // There's no stone below us, so just hover 1 space above the board
          };
        make_stack_elements(carry, cur_loc.x(), cur_loc.y(), base_z)?;
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
      let board_size = board_size.get() as i32;

      let window = web_sys::window().expect("Couldn't get window");
      let document = window.document().expect("Couldn't get document");

      let files = document.get_element_by_id("files").expect("Couldn't get files div");
      let ranks = document.get_element_by_id("ranks").expect("Couldn't get ranks div");
      let board_wrapper = document.get_element_by_id("board-wrapper").expect("Couldn't get board-wrapper div");
      let board_wrapper: HtmlElement = board_wrapper.dyn_into()?;

      let files_height = files.children().item(0).unwrap().client_height();
      let ranks_width = ranks.client_width();
      let square_size = (board_wrapper.offset_height() - files_height) / (board_size + 1);
      let target_width = square_size * board_size + ranks_width;

      board_wrapper.style().set_property("width", &(target_width.to_string()))?;

      Ok(())
    })()
      .unwrap_or_else(|e| console_log!("Display::adjust_board_width error: {:?}", e));
  }
}
