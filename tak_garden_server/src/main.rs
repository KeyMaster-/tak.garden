use std::collections::HashMap;
use std::sync::Arc;
use std::default::Default;

// use std::str::FromStr;
use tokio::sync::{mpsc, RwLock};

use futures::{FutureExt, StreamExt};
use warp::ws::{WebSocket, Message};
use warp::{Filter, Reply};

use rustak::{BoardSize, Color, Move, MoveHistory};
use tak_garden_common::{ServerMessage, ClientMessage};

use harsh::{Harsh, HarshBuilder};

type ConnectionID = u64;
const NO_CONNECTION_ID: ConnectionID = 0;
// static NEXT_CONNECTION_ID: ConnectionID = AtomicUsize::new(NO_CONNECTION_ID + 1);

  // Needs to be a result due to the unbounded sender -> websocket forwarding
  // Unsure what exactly the reasoning is.
type ConnectionSender = mpsc::UnboundedSender<Result<Message, warp::Error>>;
type ConnectionsRef = Arc<RwLock<HashMap<ConnectionID, ConnectionSender>>>;

type MatchID = u64;

// TODO consider renaming Match and Matches, because variables of those types
// end up easily clashing with the `match` keywoard and the `matches` macro
struct Match {
  id_hash: String, // TODO think more about the hash <-> id dance and where to best store this
  history: MoveHistory,
  controllers: (ConnectionID, ConnectionID),
  connections: HashMap<ConnectionID, ConnectionSender>,
}

impl Match {
  fn new(id_hash: String, size: BoardSize) -> Self {
    Self {
      id_hash,
      history: MoveHistory::new(size),
      controllers: (NO_CONNECTION_ID, NO_CONNECTION_ID),
      connections: HashMap::new(),
    }
  }

    // Adds the connection to the match. Returns what color this connection controls, if any
  fn add_connection(&mut self, id: ConnectionID, sender: ConnectionSender) -> Option<Color> {
    self.connections.insert(id, sender);

    if self.controllers.0 == NO_CONNECTION_ID {
      self.controllers.0 = id;
      Some(Color::White)
    } else if self.controllers.1 == NO_CONNECTION_ID {
      self.controllers.1 = id;
      Some(Color::Black)
    } else {
      None
    }
  }

  fn remove_connection(&mut self, id: ConnectionID) {
    self.connections.remove(&id);
    if self.controllers.0 == id {
      self.controllers.0 = NO_CONNECTION_ID;
    } else if self.controllers.1 == id {
      self.controllers.1 = NO_CONNECTION_ID;
    }
  }

  fn id_hash(&self) -> &str {
    &self.id_hash
  }

  fn history(&self) -> &MoveHistory {
    &self.history
  }

  fn history_mut(&mut self) -> &mut MoveHistory {
    &mut self.history
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
  let connections = ConnectionsRef::default();

  let matches = Arc::new(RwLock::new(Matches::new()));

  // Create warp filters that supply cloned refs to each of our global data structures on each ws connection
  let data = {
    let matches = matches.clone();
    warp::any().map(move || (next_connection_id.clone(), connections.clone(), matches.clone()))
  };

  let ws_connect = warp::path!("ws")
    .and(warp::ws()).and(data)
    .map(|ws: warp::ws::Ws, (next_connection_id, connections, matches)| {
      ws.on_upgrade(move |socket| on_connected(socket, next_connection_id, connections, matches))
    });

  let static_files = warp::fs::dir("dist");

  let game = warp::path::end()
    .or(match_id_hash(matches.clone()))
    .and(warp::fs::file("dist/index.html"))
    .map(|_, reply: warp::filters::fs::File| {
      reply.into_response()
    });

    // if at / or /game_id, serve index. Otherwise, serve websocket at /ws and static files at their specific paths (/filename.ext)
    // TODO either the whole chain should send a nice error response if none match, or the game filter does that
  warp::serve(game.or(ws_connect).or(static_files))
    .run(([127, 0, 0, 1], 3030)).await;
}

fn match_id_hash(matches: Arc<RwLock<Matches>>) -> impl Filter<Extract = (MatchRef,), Error = warp::reject::Rejection> + Clone {
  warp::path::param()
    .and(warp::any().map(move || matches.clone())) // This is the only way I found to provide a clone'd resource to this while keeping the and_then closure Fn and the overall result Clone
    .and_then(move |id_hash: String, matches: Arc<RwLock<Matches>>| async move {
      matches.read().await.get(&id_hash).ok_or(warp::reject::not_found())
      // decode_match_id_hash(&game_hash, &hasher).ok_or(warp::reject::not_found())
    })
}

async fn on_connected(ws: WebSocket, next_connection_id: Arc<RwLock<ConnectionID>>, connections: ConnectionsRef, matches: MatchesRef) {
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

  connections.write().await.insert(conn_id, tx.clone());

    // ID of the match this connection is in
  let mut match_ref: Option<MatchRef> = None;

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
          broadcast_match_state(&match_ref).await;
        }
        // *match_state.write().await = MoveHistory::new(size);
        // broadcast_match_state(&match_state, &connections).await;
      },
      ClientMessage::UndoMove => {
        if let Some(ref match_ref) = match_ref {
          println!("Undoing last move");
          match_ref.write().await.history_mut().undo();
          broadcast_match_state(&match_ref).await;
        }
      }
    }
  }

  on_disconnected(match_ref, conn_id, &connections).await;
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

  let controlled_color = match_ref.write().await.add_connection(conn_id, sender.clone());

  {
    let m = match_ref.read().await;
    send_match_state(&sender, m.id_hash(), m.history());
  }
  send_msg(sender, &ServerMessage::Control(controlled_color));

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

    let is_active_player = match current_state.active_color() {
      Color::White => conn_id == match_state.controllers.0,
      Color::Black => conn_id == match_state.controllers.1,
    };

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
      broadcast_match_state(match_state).await;
    }
  } else {
    send_msg(&sender, &ServerMessage::ActionInvalid("It's not your turn.".to_string()));
  }
}

fn send_match_state(tx: &ConnectionSender, match_hash: &str, history: &MoveHistory) {
  let msg = ServerMessage::GameState(match_hash.to_string(), history.moves().to_vec(), history.size());
  send_msg(tx, &msg);
}

async fn broadcast_match_state(match_state: &MatchRef) {
  let match_state = match_state.read().await;
  let history = match_state.history();
  let msg = ServerMessage::GameState(match_state.id_hash().to_string(), history.moves().to_vec(), history.size());

  for tx in match_state.connections.values() {
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

async fn on_disconnected(match_ref: Option<MatchRef>, conn_id: ConnectionID, connections: &ConnectionsRef) {
  if let Some(match_ref) = match_ref {
    match_ref.write().await.remove_connection(conn_id);
  }
  connections.write().await.remove(&conn_id);
}
