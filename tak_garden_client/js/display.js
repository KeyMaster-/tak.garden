const input = document.getElementById('input');
const output = document.getElementById('output');
const board_wrapper = document.getElementById('board-wrapper');
const board = document.getElementById('board');
const spaces = document.getElementById('spaces');
const stones = document.getElementById('stones');
const ranks = document.getElementById('ranks');
const files = document.getElementById('files');

const stone_counter_light_flat = document.getElementById('stone-counter-light-flat');
const stone_counter_light_cap  = document.getElementById('stone-counter-light-cap');
const stone_counter_dark_flat  = document.getElementById('stone-counter-dark-flat');
const stone_counter_dark_cap   = document.getElementById('stone-counter-dark-cap');

const game_status = document.getElementById('game_status');
const player_status = document.getElementById('player_status');

let client = null;

class Display {
  constructor() {
    this.client = null;
    this._is_connected = false;

    input.addEventListener("keyup", event => {
      if (event.keyCode === 13) {
        event.preventDefault();
        const move_text = input.value;
        this.client.submit_action(move_text);
        input.value = '';
      }
    });

    this.board_size = null;
    window.addEventListener("resize", (e) => {
      this.adjustBoardWidth();
    });
  }

  adjustBoardWidth() {
    if (this.client != null) {
      this.client.adjust_board_width();
    }
  }

  set_client(client) {
    this.client = client;
  }

  get is_connected() {
    return this._is_connected;
  }
  set is_connected(v) {
    this._is_connected = v;
    if (v) {
      output.innerText = "Connected.";
    } else {
      output.innerText = "Disconnected.";
    }
  }
}

export {
  Display
}