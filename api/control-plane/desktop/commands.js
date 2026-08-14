import { handleDesktopCommands } from "../../../relay/handlers.js";
import { getRelayStore } from "../../../relay/store.js";

export default function commands(request, response) {
  return handleDesktopCommands(request, response, getRelayStore());
}
