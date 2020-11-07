import { Client } from "../out/pkg/tak_garden_client";
import { Display } from "./display.js";
import { Connection } from "./connection.js";

let display = new Display();
let connection = new Connection(display);
let client = new Client(connection);
display.set_client(client);
connection.set_client(client);

connection.open();