use rustak::{Color, Move, BoardSize};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ServerMessage {
  Control(MatchControl), // TODO combine this into the game state message
  ActionInvalid(String), // with reason message
  GameState(String, Vec<Move>, BoardSize) // initial string is the match ID hash
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ClientMessage {
  JoinMatch(Option<String>), // JoinMatch(None) requests a new game
  Move(Move),
  ResetGame(BoardSize),
  UndoMove
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum MatchControl {
  Both,
  Single(Color),
  None
}

impl MatchControl {
  pub fn controls(&self, color: Color) -> bool {
    match self {
      MatchControl::Both => true,
      MatchControl::Single(self_color) => color == *self_color,
      MatchControl::None => false
    }
  }
}