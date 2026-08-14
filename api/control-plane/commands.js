import { handleCommands } from "../../relay/handlers.js";
import { getRelayStore } from "../../relay/store.js";

export default function commands(request, response) {
  return handleCommands(request, response, getRelayStore());
}
