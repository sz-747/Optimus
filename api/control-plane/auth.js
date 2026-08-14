import { handleAuth } from "../../relay/handlers.js";
import { getRelayStore } from "../../relay/store.js";

export default function auth(request, response) {
  return handleAuth(request, response, getRelayStore());
}
