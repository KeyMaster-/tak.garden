use rustak::{Color, Move, BoardSize};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ServerMessage {
  Control(Option<Color>), // TODO combine this into the game state message
  ActionInvalid(String), // with reason message
  GameState(Vec<Move>, BoardSize)
}
#[derive(Serialize, Deserialize, Debug)]
pub enum ClientMessage {
  Move(Move),
  ResetGame(BoardSize),
  UndoMove,
  SwapPlayers
}