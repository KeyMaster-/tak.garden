class Connection {
  constructor(display) {
    this.ws = null;
    this.display = display;
  }

  set_client(client) {
    this.client = client;
  }

  open() {
    const uri = 'wss://' + location.host + '/ws';
    this.ws = new WebSocket(uri);

    this.ws.onopen = () => {
      this.display.is_connected = true;
    };

    this.ws.onclose = () => {
      this.display.is_connected = false;
    };

    this.ws.onmessage = event => {
      if (event.data instanceof Blob) {
        event.data.arrayBuffer().then(buffer => this.client.on_message(new Uint8Array(buffer)));
      }
    };
  }

  send_message(msg) {
    this.ws.send(msg);
  }
}

export {
  Connection
}