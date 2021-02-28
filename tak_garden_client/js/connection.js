class Connection {
  constructor(display) {
    this.ws = null;
    this.display = display;
  }

  set_client(client) {
    this.client = client;
  }

  open() {
      // pathname should be /<game id>
      // TODO add verification? or do we just let the server handle that?
    const uri = 'wss://' + location.host + '/ws' + window.location.pathname;
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