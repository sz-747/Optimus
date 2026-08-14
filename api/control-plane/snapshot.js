import { handleSnapshot } from "../../relay/handlers.js";
import { getRelayStore } from "../../relay/store.js";

export default function snapshot(request, response) {
  return handleSnapshot(request, response, getRelayStore());
}
