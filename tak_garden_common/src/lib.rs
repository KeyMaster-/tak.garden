use rustak::{Color, Move, BoardSize};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ServerMessage {
  Control(Option<Color>), // TODO combine this into the game state message
  ActionInvalid(String), // with reason message
  GameState(String, Vec<Move>, BoardSize) // initial string is the match ID
}
#[derive(Serialize, Deserialize, Debug)]
pub enum ClientMessage {
  JoinMatch(Option<String>), // JoinMatch(None) requests a new game
  Move(Move),
  ResetGame(BoardSize),
  UndoMove
}