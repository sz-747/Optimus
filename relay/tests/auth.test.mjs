import assert from "node:assert/strict";
import test from "node:test";
import { authorizeDashboard, authorizeDesktop, createDashboardCookie, verifyDashboardToken } from "../auth.js";

const dashboardToken = "dashboard-token-at-least-24-characters";
const desktopToken = "desktop-token-at-least-24-characters";

test.beforeEach(() => {
  process.env.OPTIMUS_DASHBOARD_TOKEN = dashboardToken;
  process.env.OPTIMUS_DESKTOP_TOKEN = desktopToken;
});

test("dashboard token becomes an HttpOnly signed cookie", () => {
  const loginRequest = request({ host: "localhost:4174" });
  const cookie = createDashboardCookie("primary", loginRequest, 1_000);
  assert.match(cookie, /HttpOnly/);
  assert.match(cookie, /SameSite=Strict/);
  assert.doesNotMatch(cookie, new RegExp(dashboardToken));
  const session = authorizeDashboard(request({ cookie: cookie.split(";")[0] }), 2_000);
  assert.equal(session.deviceId, "primary");
});

test("dashboard and desktop credentials cannot cross roles", () => {
  assert.equal(verifyDashboardToken(dashboardToken), true);
  assert.equal(verifyDashboardToken(desktopToken), false);
  assert.throws(() => authorizeDesktop(request({ authorization: `Bearer ${dashboardToken}`, "x-optimus-device": "primary" })), /authentication required/);
  assert.equal(authorizeDesktop(request({ authorization: `Bearer ${desktopToken}`, "x-optimus-device": "primary" })), "primary");
});

test("query-string or malformed credentials are rejected", () => {
  assert.throws(() => authorizeDesktop({ ...request({ "x-optimus-device": "primary" }), url: `/?token=${desktopToken}` }), /authentication required/);
  assert.throws(() => authorizeDesktop(request({ authorization: desktopToken, "x-optimus-device": "primary" })), /authentication required/);
});

function request(headers = {}) {
  return { headers, socket: { remoteAddress: "127.0.0.1" } };
}
