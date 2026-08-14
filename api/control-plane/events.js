import { handleEvents } from "../../relay/handlers.js";
import { getRelayStore } from "../../relay/store.js";

export default function events(request, response) {
  return handleEvents(request, response, getRelayStore());
}
