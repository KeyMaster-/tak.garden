use std::collections::HashMap;
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  Arc
};
// use std::str::FromStr;
use tokio::sync::{mpsc, RwLock};

use futures::{FutureExt, StreamExt};
use warp::ws::{WebSocket, Message};
use warp::{Filter, Reply, http::Uri};

use rustak::{BoardSize, Color, Move, MoveHistory};
use tak_garden_common::{ServerMessage, ClientMessage};

use harsh::{Harsh, HarshBuilder};

const NO_CONNECTION_ID: usize = 0;
static NEXT_CONNECTION_ID: AtomicUsize = AtomicUsize::new(NO_CONNECTION_ID + 1);

type MatchState = Arc<RwLock<MoveHistory>>;
  // Needs to be a result due to the unbounded sender -> websocket forwarding
  // Unsure what exactly the reasoning is.
type ConnectionSender = mpsc::UnboundedSender<Result<Message, warp::Error>>;
type Connections = Arc<RwLock<HashMap<usize, ConnectionSender>>>;
type ControllerIDs = Arc<(AtomicUsize, AtomicUsize)>;

// #[derive(Debug)]
// struct GameId(String);

// TODO consider using the FromStr + a lazy static to get at the hasher, instead of doing and_then filter backflips
// impl FromStr for GameId {
//   type Err = ();

//   fn from_str(s: &str) -> Result<Self, Self::Err> {
//     if s == "abcdef" {
//       Ok(Self(s.to_string()))
//     } else {
//       Err(())
//     }
//   }
// }

#[tokio::main]
async fn main() {
  let match_state = Arc::new(RwLock::new(MoveHistory::new(BoardSize::new(5).unwrap())));
    // make a filter that provides a reference to our game state
  let match_state = warp::any().map(move || match_state.clone());

  let connections = Connections::default();
  let connections = warp::any().map(move || connections.clone());

  let controller_ids = Arc::new((AtomicUsize::new(NO_CONNECTION_ID), AtomicUsize::new(NO_CONNECTION_ID)));
  let controller_ids = warp::any().map(move || controller_ids.clone());

  let hasher = Arc::new(HarshBuilder::new().salt("tak.garden").length(6).build().expect("Couldn't construct a hasher."));

  let next_game_id = Arc::new(RwLock::new(1_u64));
  let next_game_id = warp::any().map(move || next_game_id.clone());

  let ws_connect = warp::path("ws")
    .and(warp::ws())
    .and(game_hash_to_id(hasher.clone()))
    .and(match_state)
    .and(connections)
    .and(controller_ids)
    .map(|ws: warp::ws::Ws, game_id, match_state, connections, controller_ids| {
      ws.on_upgrade(move |socket| on_connected(socket, match_state, connections, controller_ids))
    });

  let static_files = warp::fs::dir("dist");

  let hasher2 = hasher.clone();
  let root = warp::path::end()
    .and(next_game_id) // TODO this and the hasher can be combined into a single "generate next id hash" function to pass in
    .and(warp::any().map(move || hasher2.clone()))
    .and_then(|next_game_id: Arc<RwLock<u64>>, hasher: Arc<Harsh>| async move {
      let game_id = {
        let mut next_id = next_game_id.write().await;
        let game_id = *next_id;
        *next_id += 1;

        game_id
      };

      let game_hash = hasher.encode(&[game_id]);
      let uri: Uri = format!("/{}", game_hash).parse().expect("/<game id> wasn't a valid uri");

        // The async block needs to know its full type, and since we never return the Err variant, it doesn't know it's error type
        // There's no easy way to annotate the type of the async block itself, so instead we're being explicit about the full type that this Ok comes from here,
        // which gives type inference all the info it needs.
      Ok::<Uri, warp::reject::Rejection>(uri)
    })
    .map(|uri| {
      warp::redirect::temporary(uri)
    });

    // TODO understand the logic of the file filter.
    // The intent of this is to match the game id, then serve index.html anyway, and somehow tell the client what game ID to connect to via WS\
    // Maybe the game id should be part of the ws path? would mean conn reset on game change
  let game = game_hash_to_id(hasher.clone())
    .and(warp::fs::file("dist/index.html"))
    .map(|_, reply: warp::filters::fs::File| {
      reply.into_response()
    });

    // if at /, reroute to /<game id>, if at /<game id>, serve index.html
    // then also serve static files and ws connection
    // execute filters in order of specificity (not sure if this matters)
    // TODO either the whole chain should send a nice error response if none match, or the game filter does that
  warp::serve(root.or(game).or(ws_connect).or(static_files))
    .run(([127, 0, 0, 1], 3030)).await;
}

fn game_hash_to_id(hasher: Arc<Harsh>) -> impl Filter<Extract = (u64,), Error = warp::reject::Rejection> + Clone {
  warp::path::param()
    .and(warp::any().map(move || hasher.clone())) // This is the only way I found to provide a clone'd resource to this while keeping the and_then closure Fn and the overall result Clone
    .and_then(move |game_hash: String, hasher: Arc<Harsh>| async move {
        // TODO make this more robust, any ID that _could_ have hashed to a valid integer would be passed through here
        // should also check against the games we actually have in memory (/ in the db)
      if let Ok(id_vec) = hasher.decode(game_hash) {
        if id_vec.len() == 1 {
          Ok(id_vec[0])
        } else {
          Err(warp::reject::not_found())
        }
      } else {
        Err(warp::reject::not_found())
      }
    })
}

async fn on_connected(ws: WebSocket, match_state: MatchState, connections: Connections, controller_ids: ControllerIDs) {
    // Since mpsc senders don't implement Eq, we need an id to associate with each
    // connection to be able to store and later remove them in a collection
  let my_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);

  let (ws_tx, mut ws_rx) = ws.split();

    // make an mpsc channel that routes to our websocket
    // this is necessary since we need to give ever connected user's task
    // access to writing to the websockets of all connected users, when they update state.
  let (tx, rx) = mpsc::unbounded_channel();
    // create another task that takes anything sent to our channel, and fowards it to the websocket
  tokio::task::spawn(rx.forward(ws_tx).map(|result| {
    if let Err(e) = result {
      eprintln!("WebSocket send error: {}", e);
    }
  }));

  let tx_2 = tx.clone();
  connections.write().await.insert(my_id, tx);

    // TODO read up on ordering and figure out if it matters here
  let is_white_controller = controller_ids.0.compare_exchange(NO_CONNECTION_ID, my_id, Ordering::Relaxed, Ordering::Relaxed).is_ok();
  let is_black_controller = if !is_white_controller {
    controller_ids.1.compare_exchange(NO_CONNECTION_ID, my_id, Ordering::Relaxed, Ordering::Relaxed).is_ok()
  } else {
    false
  };

  let controlled_color = if is_white_controller {
    Some(Color::White)
  } else if is_black_controller {
    Some(Color::Black)
  } else {
    None
  };

  // send initial state
  send_match_state(&tx_2, &match_state).await;
  send_msg(&tx_2, &ServerMessage::Control(controlled_color));

  while let Some(res) = ws_rx.next().await {
    let msg = match res {
      Ok(msg) => msg,
      Err(e) => {
        eprintln!("WebSocket error: {}", e);
        break;
      }
    };

    if !msg.is_binary() {
      println!("Got message {:?}, but it's not binary. Ignoring.", msg);
      continue;
    }
    let bytes = msg.as_bytes();
    let deser_res: Result<ClientMessage, _> = serde_cbor::de::from_slice(bytes);
    if let Err(e) = deser_res {
      println!("Got invalid message: {:?}, error {:?}", msg, e);
      // TODO feedback to client?
      continue;
    }

    let client_msg = deser_res.unwrap();
    match client_msg {
      ClientMessage::Move(m) => on_move(my_id, m, &match_state, &connections, &tx_2, &controller_ids).await,
      ClientMessage::ResetGame(size) => {
        *match_state.write().await = MoveHistory::new(size);
        broadcast_match_state(&match_state, &connections).await;
      },
      ClientMessage::UndoMove => {
        println!("Undoing last move");
        {
          let ref mut history = *match_state.write().await;
          history.undo();
        }
        broadcast_match_state(&match_state, &connections).await;
      }
    }
  }

  on_disconnected(my_id, &connections, &controller_ids).await;
}

async fn on_move(conn_id: usize, m: Move, match_state: &MatchState, connections: &Connections, connection_tx: &ConnectionSender, controller_ids: &ControllerIDs) {
  let move_res = {
      // Need to get write access here before we're even sure whether we're allowed to make a move.
      // This is because the decision depends on the active color, which is part of the game state.
      // If we only got read access to do the check first, as soon as we'd give up read access to
      // do a subsequent write, someone else might get write access before us, and then we might
      // not be allowed to make the move anymore.
      // So instead, we get write access _in case_ we need to make a move, then check, and conditionally
      // do or don't do it.

      // Write access is constrained to this block, so that later on we can read the game state
      // for broadcasting it.
    let ref mut history = *match_state.write().await;
    let current_state = history.last();

    let active_color = current_state.active_color();
    let is_color_controller = match active_color {
      Color::White => conn_id == controller_ids.0.load(Ordering::Relaxed),
      Color::Black => conn_id == controller_ids.1.load(Ordering::Relaxed),
    };

    if is_color_controller {
      println!("Move: {}, {}", active_color, m);
      Some(history.add(m))
    } else {
      None
    }
  };

  if let Some(move_res) = move_res {
    if let Err(reason) = move_res {
      send_msg(connection_tx, &ServerMessage::ActionInvalid(format!("{}", reason)));
    } else {
      broadcast_match_state(match_state, connections).await;
    }
  } else {
    send_msg(connection_tx, &ServerMessage::ActionInvalid("It's not your turn.".to_string()));
  }
}

async fn send_match_state(tx: &ConnectionSender, match_state: &MatchState) {
  let (moves, size) = {
    let history = match_state.read().await;
    (history.moves().to_vec(), history.size())
  };
  let msg = ServerMessage::GameState(moves, size);
  send_msg(tx, &msg);
}

async fn broadcast_match_state(match_state: &MatchState, connections: &Connections) {
  let (moves, size) = {
    let history = match_state.read().await;
    (history.moves().to_vec(), history.size())
  };
  let msg = ServerMessage::GameState(moves, size);

  for (_, tx) in connections.read().await.iter() {
    send_msg(tx, &msg);
  }
}

fn send_msg(tx: &ConnectionSender, msg: &ServerMessage) {
  let msg_binary_res = serde_cbor::ser::to_vec_packed(msg);
  if let Err(e) = msg_binary_res {
    println!("Failed to serialise {:?}: {}", msg, e);
    return;
  }
  let msg_binary = msg_binary_res.unwrap();
  if let Err(_disconnected) = tx.send(Ok(Message::binary(msg_binary))) {
    // Handled in on_disconnected
  };
}

async fn on_disconnected(my_id: usize, connections: &Connections, controller_ids: &ControllerIDs) {
  connections.write().await.remove(&my_id);
    // remove ourselves as the white or black controller, if we were either
  controller_ids.0.compare_exchange(my_id, NO_CONNECTION_ID, Ordering::Relaxed, Ordering::Relaxed)
    .err().map(|current| if current == my_id { println!("Failed to remove {} as the white player.", my_id);});
  controller_ids.1.compare_exchange(my_id, NO_CONNECTION_ID, Ordering::Relaxed, Ordering::Relaxed)
    .err().map(|current| if current == my_id { println!("Failed to remove {} as the black player.", my_id);});
}
