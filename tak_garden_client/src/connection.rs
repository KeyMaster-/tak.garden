use crate::{Client, console_log};
use std::rc::Weak;
use std::cell::RefCell;
use web_sys::{WebSocket, MessageEvent, BinaryType};
use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[derive(Debug)]
pub struct Connection {
  websocket: WebSocket,
  // We're holding these only for ownership, not for usage
  _client: Weak<RefCell<Client>>,
  _open_callback: Closure<dyn Fn()>,
  _message_callback: Closure<dyn Fn(MessageEvent)>,
}

impl Connection {
  pub fn new(client: Weak<RefCell<Client>>) -> Result<Self, JsValue> {
    let domain = web_sys::window().expect("Couldn't get window").location().host()?;
    let url = format!("wss://{}/ws", domain);
    let websocket = WebSocket::new(&url)?;
    websocket.set_binary_type(BinaryType::Arraybuffer);

    let open_callback = {
      let client = client.clone();
      let callback = Closure::wrap(Box::new(move || {
        client.upgrade().unwrap().borrow().on_connected();
      }) as Box<dyn Fn()>);

      websocket.set_onopen(Some(callback.as_ref().unchecked_ref()));
      callback
    };

    let message_callback = {
      let client = client.clone();
      let callback = Closure::wrap(Box::new(move |event: MessageEvent| {
        if let Ok(buffer) = event.data().dyn_into::<ArrayBuffer>() {
          let array = Uint8Array::new(&buffer);
          let bytes = array.to_vec();
          client.upgrade().unwrap().borrow_mut().on_message(&bytes);
        } else {
          console_log!("Received unknown websocket message type: {:?}", event.data());
        }
      }) as Box<dyn Fn(MessageEvent)>);
      websocket.set_onmessage(Some(callback.as_ref().unchecked_ref()));
      callback
    };


    Ok(Connection {
      websocket,
      _client: client,
      _open_callback: open_callback,
      _message_callback: message_callback,
    })
  }

  pub fn send_message(&self, msg: &[u8]) {
    self.websocket.send_with_u8_array(msg)
      .unwrap_or_else(|e| console_log!("WebSocket send error: {:?}", e));
  }
}