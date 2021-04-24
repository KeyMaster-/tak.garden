// Import the client wasm module via a JS binding file.
// This will call our Rust `start()` function, which initialises the app state etc.
import("../out/pkg/tak_garden_client")
  .catch(console.error);
