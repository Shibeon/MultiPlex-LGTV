// The Watch Together relay on loopback, for the native end-to-end test
// (`rust-modules/src/app/adapters/together_e2e_tests.rs`). It runs the REAL relay (`createRelay` of
// the web repository's `apps/relay/server.mjs`, path given as the first argument) in front of two
// stand-ins answering as the real services do on the only addresses the relay uses, as the relay's
// own `friends.test.mjs` does:
// - its Plex server: /identity, /library/sections (200 with one of its users' tokens, 401 otherwise),
//   /status/sessions (the owner only);
// - plex.tv, with the owner's token: /api/v2/user, /api/servers/{id}/shared_servers, /api/users/.
// Nothing here reaches a real server or a real account. Prints one JSON line with the relay's port,
// then runs until its standard input closes.
import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';

const relayPath = process.argv[2];
if (!relayPath) {
  console.error('usage: node together_relay_harness.mjs <path to apps/relay/server.mjs>');
  process.exit(2);
}
const { createRelay } = await import(pathToFileURL(relayPath).href);

export const SERVER_ID = 'mock-machine-identifier-0001';
const PLEX_TOKEN = 'OWNER-PLEXTV-ACCOUNT-TOKEN-mock';
const OWNER_ID = '1000';
// Shared users of the mock server: [plex.tv id, title, username, the server's own token for them].
const USERS = [
  ['2001', 'Camille', 'camille', 'friend-server-token'],
  ['2002', 'Dominique', 'dom', 'host-server-token'],
  ['2003', 'Eli', 'eli', 'other-server-user-token'],
];

const listen = (srv) => new Promise((r) => srv.listen(0, '127.0.0.1', () => r(`http://127.0.0.1:${srv.address().port}`)));

const plex = createServer((req, res) => {
  const token = req.headers['x-plex-token'];
  if (req.url === '/identity') {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ MediaContainer: { size: 0, claimed: true, machineIdentifier: SERVER_ID, version: '1.43.4' } }));
  }
  const known = USERS.some((u) => u[3] === token);
  const allowed = req.url === '/library/sections' ? known : req.url === '/status/sessions' ? false : null;
  if (allowed === null) {
    res.writeHead(404);
    return res.end();
  }
  res.writeHead(allowed ? 200 : 401, { 'content-type': 'application/json' });
  res.end(allowed ? JSON.stringify({ MediaContainer: { size: 0 } }) : '');
});

const plexTv = createServer((req, res) => {
  if (req.headers['x-plex-token'] !== PLEX_TOKEN) {
    res.writeHead(401);
    return res.end();
  }
  if (req.url === '/api/v2/user') {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ id: Number(OWNER_ID), uuid: 'u-owner', username: 'proprio', title: 'Proprio', thumb: '' }));
  }
  if (req.url === `/api/servers/${SERVER_ID}/shared_servers`) {
    res.writeHead(200, { 'content-type': 'application/xml' });
    const rows = USERS.map(([id, , , t], i) => `<SharedServer id="${90 + i}" userID="${id}" accessToken="${t}" serverId="7" machineIdentifier="${SERVER_ID}"/>`);
    return res.end(`<?xml version="1.0" encoding="UTF-8"?>\n<MediaContainer machineIdentifier="${SERVER_ID}" size="${rows.length}">${rows.join('')}</MediaContainer>`);
  }
  if (req.url === '/api/users/') {
    res.writeHead(200, { 'content-type': 'application/xml' });
    const rows = USERS.map(([id, title, username]) => `<User id="${id}" title="${title}" username="${username}" thumb="" home="0"/>`);
    return res.end(`<?xml version="1.0" encoding="UTF-8"?>\n<MediaContainer size="${rows.length}">${rows.join('')}</MediaContainer>`);
  }
  res.writeHead(404);
  res.end();
});

const plexUrl = await listen(plex);
const plexTvUrl = await listen(plexTv);
const relay = createRelay({ plexUrl, plexTvUrl, plexToken: PLEX_TOKEN, ipBurst: 500, ipPerSecond: 50 });
await relay.probing;
await new Promise((r) => relay.http.listen(0, '127.0.0.1', r));
console.log(JSON.stringify({ port: relay.http.address().port, server: SERVER_ID }));

process.stdin.resume();
process.stdin.on('end', () => process.exit(0));
process.stdin.on('close', () => process.exit(0));
