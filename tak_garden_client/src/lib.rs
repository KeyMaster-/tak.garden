mod utils;

use wasm_bindgen::prelude::*;
use tak_garden_common::{ServerMessage, ClientMessage};
use rustak::{Color, BoardSize, Game};

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

#[wasm_bindgen(module = "/js/display.js")]
extern "C" {
  pub type Display;

  #[wasm_bindgen(method)]
  fn set_action_response(this: &Display, msg: &str);
  #[wasm_bindgen(method)]
  fn set_player_status(this: &Display, msg: &str);

  #[wasm_bindgen(method)]
  fn set_game_state(this: &Display, state: JsValue);
}

#[wasm_bindgen(module = "/js/connection.js")]
extern "C" {
  pub type Connection;

  #[wasm_bindgen(method)]
  fn send_message(this: &Connection, msg: &[u8]);
}

#[wasm_bindgen]
pub struct Client {
  display: Display,
  connection: Connection,
  game: Option<Game>
}

#[wasm_bindgen]
impl Client {
  #[wasm_bindgen(constructor)]
  pub fn new(display: Display, connection: Connection) -> Self {
    Self {
      display,
      connection,
      game: None
    }
  }

  pub fn on_message(&mut self, msg: &[u8]) {
    let res = self.on_message_internal(msg);
    if let Err(e) = res {
      console_log!("Error processing the game message {:?}: {:?}", msg, e);
    }
  }

  fn on_message_internal(&mut self, msg: &[u8]) -> Result<(), ()> { // TODO better error type
    let msg: ServerMessage = serde_cbor::de::from_slice(msg).map_err(|_| ())?;
    match msg {
      ServerMessage::Control(color_opt) => self.display.set_player_status(&control_message(color_opt)),
      ServerMessage::ActionInvalid(reason) => self.display.set_action_response(&reason),
      ServerMessage::GameState(game) => {
        self.game = Some(game);

        // TODO hand-rolled rust->JsValue conversion based on this serde code
        // Compact down stone kind/color enums, turn the grid into a 2d array for nicer access
        let js_game_state = serde_wasm_bindgen::to_value(self.game.as_ref().unwrap());
        if let Ok(js_game_state) = js_game_state {
          self.display.set_game_state(js_game_state);
        }
      }
    }
    Ok(())
  }

  pub fn submit_action(&self, msg: &str) {
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
      self.submit_move(msg);
    }
  }

  fn submit_move(&self, msg: &str) { // TODO maybe signal to the display whether the move was okay or not through a return value?
    let move_parse_res = msg.parse();
    if let Ok(m) = move_parse_res {
      self.send_message(&ClientMessage::Move(m));
    } else {
      //TODO tell display that the submitted move is wrong
    }

    // parse into move struct
    // send error back to display if appropriate
    // else serialise, pass binary message to connection struct
      // need connection class, contains ws connection, created in index.js,
      // handles forwarding to client and receiving from client
  }

  fn send_message(&self, msg: &ClientMessage) {
    let msg_binary_res = serde_cbor::ser::to_vec_packed(msg);
    if let Err(e) = msg_binary_res {
      console_log!("Failed to serialise {:?}: {}", msg, e);
      return;
    }

    self.connection.send_message(&msg_binary_res.unwrap());
  }
}

fn control_message(c: Option<Color>) -> String {
  let color_str = match c {
    Some(color) => color.to_string(),
    None => "No color".to_string()
  };
  format!("You control: {}", color_str)
}
