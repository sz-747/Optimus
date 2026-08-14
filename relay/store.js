import { UpstashRelayStore } from "./upstash-store.js";

let singleton;

export function getRelayStore() {
  singleton ||= new UpstashRelayStore();
  return singleton;
}
