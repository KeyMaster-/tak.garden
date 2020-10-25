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
    if (this.board_size != null) {
      let files_height = files.children[0].clientHeight;
      let ranks_width = ranks.clientWidth;
      let square_size = (board_wrapper.offsetHeight - files_height) / (this.board_size + 1);
      let target_width = square_size * this.board_size + ranks_width

      board_wrapper.style.width = target_width;
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

  set_action_response(msg) {
    output.innerText = msg;
    // TODO consolidate callsites for this logic.
    // This is here because we might not have had text in the output before, in which case this changes the available height for the board
    this.adjustBoardWidth(); 
  }

  set_player_status(msg) {
    player_status.innerText = msg;
    // TODO consolidate callsites for this logic. See above.
    this.adjustBoardWidth();
  }

  set_game_state(state) {
    this.board_size = Math.sqrt(state.board.length);

    while (spaces.lastElementChild) {
      spaces.removeChild(spaces.lastElementChild);
    }

    board_wrapper.className = "";
    board_wrapper.classList.add("board-wrapper");
    board_wrapper.classList.add("size-" + this.board_size);

    // create spaces
    for(let row = 0; row < this.board_size; row++) {
      for(let col = 0; col < this.board_size; col++) {
        let color_class = (col + (row % 2)) % 2 == 0 ? "dark" : "light";
        let space = document.createElement("div");
        space.classList.add("space");
        space.classList.add(color_class); 
        spaces.appendChild(space);
      }
    }

    while (ranks.lastElementChild) {
      ranks.removeChild(ranks.lastElementChild);
    }
    for(let rank = this.board_size; rank >= 1; rank--) {
      let rank_el = document.createElement("div");
      rank_el.innerText = rank;
      ranks.appendChild(rank_el);
    }

    while (files.lastElementChild) {
      files.removeChild(files.lastElementChild);
    }
    for(let file_idx = 0; file_idx < this.board_size; file_idx++) {
      let file_el = document.createElement("div");
      file_el.innerText = "abcdefgh"[file_idx];
      files.appendChild(file_el);
    }

    while (stones.lastElementChild) {
      stones.removeChild(stones.lastElementChild);
    }

    state.board.forEach((stack, idx, arr) => {
      let x = idx % this.board_size;
      let y = Math.floor(idx / this.board_size);
      let z = 0;

      stack.forEach((stone, idx, arr) => {
        let color_class = "";
        let stone_kind_class = "";

        switch (stone.color) {
          case "Black":
            color_class = "dark";
            break;
          case "White":
            color_class = "light";
            break;
          default:
            break;
        }
        switch (stone.kind) {
          case "FlatStone":
            // flat stones are the default, no class needed
            break;
          case "StandingStone":
            stone_kind_class = "standing";
            if (z > 0) z -= 1; // TODO this should be somewhere else.
            break;
          case "Capstone":
            stone_kind_class = "cap";
            if (z > 0) z -= 1; // TODO this should be somewhere else.
            break;
          default:
            break;
        }

        let wrapper = document.createElement("div");
        wrapper.classList.add("stone-wrapper");
        wrapper.style.transform = "translate(" + x * 100 + "%, " + (y * -100 + z * -7) + "%)";
        let stone_el = document.createElement("div");
        stone_el.classList.add("stone");
        if (stone_kind_class != "") {
          stone_el.classList.add(stone_kind_class);
        }
        stone_el.classList.add(color_class);

        wrapper.appendChild(stone_el);
        stones.appendChild(wrapper);

        z += 1;
      });
    });

    let status = "";
    if (state.state == "Draw") {
      status += "Draw. ";
    } else if (state.state.Win != undefined) { 
      status += state.state.Win[0] + " wins "; // Win[0] is "Black" or "White"
      if (state.state.Win[1] == "Road") {
        status += " by road. ";
      } else {
        status += " by flats ";
        if (state.state.Win[1] == "BoardFilled") {
          status += "(board full). ";
        } else {
            // PlayedAllStones[0] is the color that did so
          status += "(" + state.state.Win[1].PlayedAllStones[0] + " played all stones). ";
        }
      }
    }
    let turn = Math.floor(state.moves / 2) + 1;
    let color = (state.moves % 2 == 0) ? "White" : "Black";
    let status_text = status;
    if (state.state == "Ongoing") {
      status_text += "Turn " + turn + ", " + color + "'s go.";
    }
    game_status.innerText = status_text;

    stone_counter_light_flat.innerText = state.held_stones.white.flat;
    stone_counter_light_cap.innerText  = state.held_stones.white.capstone;
    stone_counter_dark_flat.innerText  = state.held_stones.black.flat;
    stone_counter_dark_cap.innerText   = state.held_stones.black.capstone;

    this.adjustBoardWidth();
  }
}

export {
  Display
}