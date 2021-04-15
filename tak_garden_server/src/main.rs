use std::collections::HashMap;
use std::sync::Arc;
use std::default::Default;

// use std::str::FromStr;
use tokio::sync::{mpsc, RwLock};

use futures::{FutureExt, StreamExt};
use warp::ws::{WebSocket, Message};
use warp::{Filter, Reply};

use rustak::{BoardSize, Color, Move, MoveHistory};
use tak_garden_common::{ServerMessage, ClientMessage, MatchControl};

use harsh::{Harsh, HarshBuilder};

type ConnectionID = u64;
const NO_CONNECTION_ID: ConnectionID = 0;
// static NEXT_CONNECTION_ID: ConnectionID = AtomicUsize::new(NO_CONNECTION_ID + 1);

  // Needs to be a result due to the unbounded sender -> websocket forwarding
  // Unsure what exactly the reasoning is.
type ConnectionSender = mpsc::UnboundedSender<Result<Message, warp::Error>>;

type MatchID = u64;

// TODO consider renaming Match and Matches, because variables of those types
// end up easily clashing with the `match` keywoard and the `matches` macro
struct Match {
  id_hash: String, // TODO think more about the hash <-> id dance and where to best store this
  history: MoveHistory,
  controllers: MatchControllers,
  connections: HashMap<ConnectionID, ConnectionSender>,
}

impl Match {
  fn new(id_hash: String, size: BoardSize) -> Self {
    Self {
      id_hash,
      history: MoveHistory::new(size),
      controllers: Default::default(),
      connections: HashMap::new(),
    }
  }

  fn add_connection(&mut self, id: ConnectionID, sender: ConnectionSender) {
    self.connections.insert(id, sender);
    if self.controllers.add(id) {
      self.broadcast_control();
    }
  }

  fn remove_connection(&mut self, id: ConnectionID) {
    self.connections.remove(&id);
    if self.controllers.remove(id) {
      self.broadcast_control();
    }
  }

  fn swap_controllers(&mut self) {
    if self.controllers.swap() {
      self.broadcast_control();
    }
  }

  fn broadcast_control(&self) {
    for &conn_id in &self.controllers {
      let sender = &self.connections[&conn_id];
      let msg = ServerMessage::Control(self.controllers.control_for(conn_id));
      send_msg(&sender, &msg);
    }
  }

  fn history(&self) -> &MoveHistory {
    &self.history
  }

  fn history_mut(&mut self) -> &mut MoveHistory {
    &mut self.history
  }

  fn controllers(&self) -> &MatchControllers {
    &self.controllers
  }

  fn state_message(&self) -> ServerMessage {
    ServerMessage::GameState(self.id_hash.clone(), self.history.moves().to_vec(), self.history.size())
  }

  fn send_to(&self, sender: &ConnectionSender) {
    send_msg(sender, &self.state_message());
  }

  fn broadcast_state(&self) {
    let msg = self.state_message();

    for tx in self.connections.values() {
      send_msg(tx, &msg);
    }
  }
}

#[derive(Debug)]
struct MatchControllers([ConnectionID; 2]);

impl MatchControllers {
  // If we have no controlling connections, our internal array is NO_CONNECTION_ID twice.
  // If we have only one controller, our internal array will contain their connection ID at index 0,
  // and NO_CONNECTION_ID at index 1. This indicates that this single connection controls both colors.
  // Else, the ids at 0 and 1 are different.
  fn add(&mut self, id: ConnectionID) -> bool {
    if self.0[0] == NO_CONNECTION_ID {
      self.0[0] = id;
      true
    } else if self.0[1] == NO_CONNECTION_ID {
      self.0[1] = id;
      true
    } else {
      false
    }
  }

  fn remove(&mut self, id: ConnectionID) -> bool {
    if self.0[0] == id {
      // If we had two controllers, moves the other to index 0, since a single controller is always stored at index 0.
      // If we were the only controller, this turns the array back into two NO_CONTROLLER_IDs
      self.0[0] = self.0[1];
      self.0[1] = NO_CONNECTION_ID;
      true
    } else if self.0[1] == id {
      self.0[1] = NO_CONNECTION_ID;
      true
    } else {
      false
    }
  }

  fn swap(&mut self) -> bool {
    if self.0[0] != NO_CONNECTION_ID && self.0[1] != NO_CONNECTION_ID {
      self.0.swap(0, 1);
      true
    } else {
      false
    }
  }

  fn control_for(&self, id: ConnectionID) -> MatchControl {
    if id == self.0[0] && self.0[1] == NO_CONNECTION_ID {
      MatchControl::Both
    } else if id == self.0[0] {
      MatchControl::Single(Color::White)
    } else if id == self.0[1] {
      MatchControl::Single(Color::Black)
    } else {
      MatchControl::None
    }
  }
}

impl Default for MatchControllers {
  fn default() -> Self {
    Self([NO_CONNECTION_ID, NO_CONNECTION_ID])
  }
}

impl<'a> IntoIterator for &'a MatchControllers {
  type Item = &'a ConnectionID;
  type IntoIter = std::slice::Iter<'a, ConnectionID>;

  fn into_iter(self) -> Self::IntoIter {
    if self.0[0] == NO_CONNECTION_ID {
      self.0[0..0].iter()
    } else if self.0[1] == NO_CONNECTION_ID {
      self.0[0..1].iter()
    } else {
      self.0.iter()
    }
  }
}

struct MatchIDHasher(Harsh);
impl MatchIDHasher {
  fn new(salt: &str, length: usize) -> Self {
    Self(HarshBuilder::new().salt(salt).length(length).build().expect("Couldn't construct a hasher."))
  }
  fn encode(&self, id: MatchID) -> String {
    self.0.encode(&[id])
  }

  fn decode(&self, hash: &str) -> Option<MatchID> {
    self.0.decode(hash).ok().and_then(|id_vec| (id_vec.len() == 1).then(|| id_vec[0]))
  }
}

impl Default for MatchIDHasher {
  fn default() -> Self {
    MatchIDHasher::new("tak.garden", 6)
  }
}

struct Matches {
  next_id: MatchID,
  hasher: MatchIDHasher,
  matches: HashMap<MatchID, MatchRef>
}

impl Matches {
  fn new() -> Self {
    Self {
      next_id: 1_u64,
      hasher: Default::default(),
      matches: HashMap::new()
    }
  }

  fn new_match(&mut self) -> MatchRef {
    let out_id = self.next_id;
    self.next_id += 1;
    let id_hash = self.hasher.encode(out_id);
    println!("Generating new game {},{}", out_id, id_hash);

    let match_ref = Arc::new(RwLock::new(Match::new(id_hash, BoardSize::new(5).unwrap())));
    self.matches.insert(out_id, match_ref.clone());
    match_ref
  }

  fn get<T>(&self, hash: T) -> Option<MatchRef> 
    where T: AsRef<str>
  {
    self.hasher.decode(hash.as_ref()).and_then(|id| self.matches.get(&id).cloned())
  }
}

type MatchRef = Arc<RwLock<Match>>;
type MatchesRef = Arc<RwLock<Matches>>;

#[tokio::main]
async fn main() {
    // TODO next connection id generation and the map can be combined into a single struct
  let next_connection_id = Arc::new(RwLock::new(NO_CONNECTION_ID + 1));

  let matches = Arc::new(RwLock::new(Matches::new()));

  // Create warp filters that supply cloned refs to each of our global data structures on each ws connection
  let data = {
    let matches = matches.clone();
    warp::any().map(move || (next_connection_id.clone(), matches.clone()))
  };

  let ws_connect = warp::path!("ws")
    .and(warp::ws()).and(data)
    .map(|ws: warp::ws::Ws, (next_connection_id, matches)| {
      ws.on_upgrade(move |socket| on_connected(socket, next_connection_id, matches))
    });

  let static_files = warp::fs::dir("dist");

  let game = warp::path::end()
    .or(match_id_hash(matches.clone()))
    .and(warp::fs::file("dist/index.html"))
    .map(|_, reply: warp::filters::fs::File| {
      reply.into_response()
    });

    // First, check if we're serving a well-known address (static files, or websocket at /ws). If so, serve that,
    // else, serve the game page (index.html) at / or /<match_id_hash>
    // TODO either the whole chain should send a nice error response if none match, or the game filter does that
  warp::serve(static_files.or(ws_connect).or(game))
    .run(([127, 0, 0, 1], 3030)).await;
}

fn match_id_hash(matches: Arc<RwLock<Matches>>) -> impl Filter<Extract = (MatchRef,), Error = warp::reject::Rejection> + Clone {
  warp::path::param()
    .and(warp::any().map(move || matches.clone())) // This is the only way I found to provide a clone'd resource to this while keeping the and_then closure Fn and the overall result Clone
    .and_then(move |id_hash: String, matches: Arc<RwLock<Matches>>| async move {
      matches.read().await.get(&id_hash).ok_or(warp::reject::not_found())
    })
}

async fn on_connected(ws: WebSocket, next_connection_id: Arc<RwLock<ConnectionID>>, matches: MatchesRef) {
    // Since mpsc senders don't implement Eq, we need an id to associate with each
    // connection to be able to store and later remove them in a collection

  let conn_id = {
    let mut next_id = next_connection_id.write().await;
    let id = *next_id;
    *next_id += 1;
    id
  };

  let (ws_tx, mut ws_rx) = ws.split();

    // make an mpsc channel that routes to our websocket
    // this is necessary since we need to give every connected user's task
    // access to writing to the websockets of all connected users, when they update state.
  let (tx, rx) = mpsc::unbounded_channel();
    // create another task that takes anything sent to our channel, and forwards it to the websocket
  tokio::task::spawn(rx.forward(ws_tx).map(|result| {
    if let Err(e) = result {
      eprintln!("WebSocket send error: {}", e);
    }
  }));

    // ID of the match this connection is in
  let mut match_ref: Option<MatchRef> = None;

  while let Some(res) = ws_rx.next().await {
    let msg = match res {
      Ok(msg) => msg,
      Err(e) => {
        // TODO Ignore specific errors that we know are harmless
        // E.g. the "connection reset without closing handshake" message is not an issue we need to log.
        eprintln!("WebSocket error: {}", e);
        break;
      }
    };

    if msg.is_close() {
      // We don't care about the contents of the close message (the close reason etc)
      // Nothing to be done here, after the close frame the stream will stop giving us results and we'll exit the loop
      continue;
    }

    if msg.is_ping() {
      // Ping messages are responded to automatically by the underlying websocket implementation,
      // but we still see them arrive as messages. We can safely ignore it.
      continue;
    }

    if !msg.is_binary() {
      // All other non-binary message types (namely text and pong messages) would be anomalous to see, so log them out.
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
      ClientMessage::JoinMatch(hash_opt) => {
          // Remove ourselves from our current match, if any. `take` turns match_ref into None along the way
        if let Some(match_ref) = match_ref.take() {
          match_ref.write().await.remove_connection(conn_id);
        }
        match_ref = Some(join_match(hash_opt, conn_id, &tx, &matches).await); // Cache the match ref so we don't have to go through the matches map each time
      },
      ClientMessage::Move(m) => {
        if let Some(ref match_ref) = match_ref {
          on_move(m, &match_ref, conn_id, &tx).await;
        }
      },//on_move(my_id, m, &match_state, &connections, &tx, &controller_ids).await,
      ClientMessage::ResetGame(size) => {
        if let Some(ref match_ref) = match_ref {
          *match_ref.write().await.history_mut() = MoveHistory::new(size);
          match_ref.read().await.broadcast_state();
        }
      },
      ClientMessage::UndoMove => {
        if let Some(ref match_ref) = match_ref {
          println!("Undoing last move");
          match_ref.write().await.history_mut().undo();
          match_ref.read().await.broadcast_state();
        }
      },
      ClientMessage::SwapPlayers => {
        if let Some(ref match_ref) = match_ref {
          match_ref.write().await.swap_controllers();
        }
      }
    }
  }

  on_disconnected(match_ref, conn_id).await;
}

// if match_id is Some, the provided MatchID is assumed to already exist.
async fn join_match<T>(match_id_hash: Option<T>, conn_id: ConnectionID, sender: &ConnectionSender, matches: &MatchesRef) -> MatchRef 
  where T: AsRef<str>
{
  let match_ref = if let Some(hash) = match_id_hash {
    matches.read().await.get(hash).expect("Match ID hash passed to join_match should be of an existing game.")
  } else {
    matches.write().await.new_match()
  };

  match_ref.write().await.add_connection(conn_id, sender.clone());
  match_ref.read().await.send_to(sender);
  match_ref
}

async fn on_move(m: Move, match_state: &MatchRef, conn_id: ConnectionID, sender: &ConnectionSender) {
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

    let ref mut match_state = *match_state.write().await;
    let current_state = match_state.history().last();

    let is_active_player = match_state.controllers().control_for(conn_id).controls(current_state.active_color());

    // let is_active_player = match current_state.active_color() {
    //   Color::White => conn_id == match_state.controllers.0,
    //   Color::Black => conn_id == match_state.controllers.1,
    // };

    if is_active_player {
      println!("Move: {}, {}", current_state.active_color(), m);
      Some(match_state.history_mut().add(m))
    } else {
      None
    }
  };

  if let Some(move_res) = move_res {
    if let Err(reason) = move_res {
      send_msg(&sender, &ServerMessage::ActionInvalid(format!("{}", reason)));
    } else {
      match_state.read().await.broadcast_state();
      // broadcast_match_state(match_state).await;
    }
  } else {
    send_msg(&sender, &ServerMessage::ActionInvalid("It's not your turn.".to_string()));
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

async fn on_disconnected(match_ref: Option<MatchRef>, conn_id: ConnectionID) {
  if let Some(match_ref) = match_ref {
    match_ref.write().await.remove_connection(conn_id);
  }
}
