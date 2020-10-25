const path = require("path");
const CopyPlugin = require("copy-webpack-plugin");
const WasmPackPlugin = require("@wasm-tool/wasm-pack-plugin");

const dist = path.resolve(__dirname, "out/dist");

module.exports = {
  mode: "development",
  devtool: 'eval-source-map',
  entry: {
    index: "./js/bootstrap.js"
  },
  output: {
    path: dist,
    filename: "bootstrap.js"
  },
  plugins: [
    new CopyPlugin([
      path.resolve(__dirname, "static")
    ]),

    new WasmPackPlugin({
      crateDirectory: __dirname,
      outDir: "out/pkg",
      outName: "tak_garden_client"
    }),
  ]
};