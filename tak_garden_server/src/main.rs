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

struct Matches {
  next_id: MatchID,
  hasher: Arc<Harsh>,
  matches: HashMap<MatchID, MatchRef>
}

impl Matches {
  fn new(hasher: Arc<Harsh>) -> Self {
    Self {
      next_id: 1_u64,
      hasher,
      matches: HashMap::new()
    }
  }

  fn new_match(&mut self) -> MatchID {
    let out_id = self.next_id;
    self.next_id += 1;
    let id_hash = self.hasher.encode(&[out_id]);
    self.matches.insert(out_id, Arc::new(RwLock::new(Match::new(id_hash, BoardSize::new(5).unwrap()))));
    out_id
  }

  fn get(&self, id: MatchID) -> MatchRef {
    self.matches[&id].clone()
  }
}

type MatchRef = Arc<RwLock<Match>>;
type MatchesRef = Arc<RwLock<Matches>>;

// type ControllerIDs = Arc<(AtomicUsize, AtomicUsize)>;

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
    // TODO next connection id generation and the map can be combined into a single struct
  let next_connection_id = Arc::new(RwLock::new(NO_CONNECTION_ID + 1));
  let connections = ConnectionsRef::default();

  let match_id_hasher = Arc::new(HarshBuilder::new().salt("tak.garden").length(6).build().expect("Couldn't construct a hasher."));
  let matches = Arc::new(RwLock::new(Matches::new(match_id_hasher.clone())));

  // let match_state = Arc::new(RwLock::new(MoveHistory::new(BoardSize::new(5).unwrap())));
  //   // make a filter that provides a reference to our game state
  // let match_state = warp::any().map(move || match_state.clone());

  // let connections = Connections::default();
  // let connections = warp::any().map(move || connections.clone());

  // let controller_ids = Arc::new((AtomicUsize::new(NO_CONNECTION_ID), AtomicUsize::new(NO_CONNECTION_ID)));
  // let controller_ids = warp::any().map(move || controller_ids.clone());

  // let next_game_id = Arc::new(RwLock::new(1_u64));
  // let next_game_id = warp::any().map(move || next_game_id.clone());

  // Create warp filters that supply cloned refs to each of our global data structures on each ws connection
  let data = {
    let match_id_hasher = match_id_hasher.clone();
    warp::any().map(move || (next_connection_id.clone(), connections.clone(), match_id_hasher.clone(), matches.clone()))
  };
  let ws_connect = warp::path!("ws")
    .and(warp::ws()).and(data)
    .map(|ws: warp::ws::Ws, (next_connection_id, connections, match_id_hasher, matches)| {
      ws.on_upgrade(move |socket| on_connected(socket, next_connection_id, connections, match_id_hasher, matches))
    });

  let static_files = warp::fs::dir("dist");

  // let hasher2 = hasher.clone();
  // let root = warp::path::end()
  //   .and(next_game_id) // TODO this and the hasher can be combined into a single "generate next id hash" function to pass in
  //   .and(warp::any().map(move || hasher2.clone()))
  //   .and_then(|next_game_id: Arc<RwLock<u64>>, hasher: Arc<Harsh>| async move {
  //     let game_id = {
  //       let mut next_id = next_game_id.write().await;
  //       let game_id = *next_id;
  //       *next_id += 1;

  //       game_id
  //     };

  //     let game_hash = hasher.encode(&[game_id]);
  //     let uri: Uri = format!("/{}", game_hash).parse().expect("/<game id> wasn't a valid uri");

  //       // The async block needs to know its full type, and since we never return the Err variant, it doesn't know it's error type
  //       // There's no easy way to annotate the type of the async block itself, so instead we're being explicit about the full type that this Ok comes from here,
  //       // which gives type inference all the info it needs.
  //     Ok::<Uri, warp::reject::Rejection>(uri)
  //   })
  //   .map(|uri| {
  //     warp::redirect::temporary(uri)
  //   });

  let game = warp::path::end()
    .or(game_hash_to_id(match_id_hasher.clone()))
    .and(warp::fs::file("dist/index.html"))
    .map(|_, reply: warp::filters::fs::File| {
      reply.into_response()
    });

    // if at / or /game_id, serve index. Otherwise, serve websocket at /ws and static files at their specific paths (/filename.ext)
    // TODO either the whole chain should send a nice error response if none match, or the game filter does that
  warp::serve(game.or(ws_connect).or(static_files))
    .run(([127, 0, 0, 1], 3030)).await;
}

fn game_hash_to_id(hasher: Arc<Harsh>) -> impl Filter<Extract = (MatchID,), Error = warp::reject::Rejection> + Clone {
  warp::path::param()
    .and(warp::any().map(move || hasher.clone())) // This is the only way I found to provide a clone'd resource to this while keeping the and_then closure Fn and the overall result Clone
    .and_then(move |game_hash: String, hasher: Arc<Harsh>| async move {
      // TODO make this more robust, any ID that _could_ have hashed to a valid integer would be passed through here
      // should also check against the games we actually have in memory (/ in the db)
      decode_match_id_hash(&game_hash, &hasher).ok_or(warp::reject::not_found())
    })
}

fn decode_match_id_hash(hash: &str, hasher: &Harsh) -> Option<MatchID> {
  hasher.decode(hash).ok().and_then(|id_vec| (id_vec.len() == 1).then(|| id_vec[0]))
}

async fn on_connected(ws: WebSocket, next_connection_id: Arc<RwLock<ConnectionID>>, connections: ConnectionsRef, match_id_hasher: Arc<Harsh>, matches: MatchesRef) {
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

  //   // TODO read up on Ordering and figure out if it matters here
  // let is_white_controller = controller_ids.0.compare_exchange(NO_CONNECTION_ID, my_id, Ordering::Relaxed, Ordering::Relaxed).is_ok();
  // let is_black_controller = if !is_white_controller {
  //   controller_ids.1.compare_exchange(NO_CONNECTION_ID, my_id, Ordering::Relaxed, Ordering::Relaxed).is_ok()
  // } else {
  //   false
  // };

  // let controlled_color = if is_white_controller {
  //   Some(Color::White)
  // } else if is_black_controller {
  //   Some(Color::Black)
  // } else {
  //   None
  // };

  // // send initial state
  // send_match_state(&tx, &match_state).await;
  // send_msg(&tx, &ServerMessage::Control(controlled_color));

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
          // Remove ourselves from our current match, if any. `take` clears out match_ref along the way
        if let Some(match_ref) = match_ref.take() {
          match_ref.write().await.remove_connection(conn_id);
        }
        let id_opt = hash_opt.and_then(|hash| decode_match_id_hash(&hash, &match_id_hasher));
        let match_id = join_match(id_opt, conn_id, &tx, &matches).await;
        match_ref = Some(matches.read().await.get(match_id)); // Cache the match ref so we don't have to go through the matches map each time
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

async fn join_match(match_id: Option<MatchID>, conn_id: ConnectionID, sender: &ConnectionSender, matches: &MatchesRef) -> MatchID {
  let match_id = if let Some(id) = match_id {
    id
  } else {
    matches.write().await.new_match()
  };

  let match_ref = matches.read().await.get(match_id);
  let controlled_color = match_ref.write().await.add_connection(conn_id, sender.clone());

  {
    let m = match_ref.read().await;
    send_match_state(&sender, m.id_hash(), m.history());
  }
  send_msg(sender, &ServerMessage::Control(controlled_color));

  match_id
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
