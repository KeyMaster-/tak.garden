import { init } from "../out/pkg/tak_garden_client";
import { Connection } from "./connection.js";

let connection = new Connection();

let client = init(connection);
connection.set_client(client);

connection.open();