// Apple's push service: HTTP/2 with a token signed by the app's APNs key (a .p8 file from the
// Apple Developer account). Nothing beyond Node's own modules.
import { createPrivateKey, sign, type KeyObject } from 'node:crypto';
import { connect, type ClientHttp2Session } from 'node:http2';
import type { PushSender } from './relay.ts';

export interface APNsKey {
  keyID: string;
  teamID: string;
  // The .p8 file's contents.
  pem: string;
}

const HOSTS = {
  production: 'https://api.push.apple.com',
  sandbox: 'https://api.sandbox.push.apple.com',
} as const;

export function apnsSender(key: APNsKey): PushSender {
  const privateKey: KeyObject = createPrivateKey(key.pem);
  const sessions = new Map<string, ClientHttp2Session>();
  let token: { value: string; made: number } | undefined;

  // Apple wants a new token at most every 20 minutes and refuses one older than an hour.
  function bearer(): string {
    const now = Math.floor(Date.now() / 1000);
    if (token && now - token.made < 50 * 60) return token.value;
    const part = (object: object) => Buffer.from(JSON.stringify(object)).toString('base64url');
    const unsigned = `${part({ alg: 'ES256', kid: key.keyID })}.${part({ iss: key.teamID, iat: now })}`;
    const signature = sign('sha256', Buffer.from(unsigned), { key: privateKey, dsaEncoding: 'ieee-p1363' });
    token = { value: `${unsigned}.${signature.toString('base64url')}`, made: now };
    return token.value;
  }

  function session(environment: keyof typeof HOSTS): ClientHttp2Session {
    const open = sessions.get(environment);
    if (open && !open.closed && !open.destroyed) return open;
    const made = connect(HOSTS[environment]);
    made.on('error', () => made.destroy());
    made.on('close', () => { if (sessions.get(environment) === made) sessions.delete(environment); });
    made.setTimeout(10 * 60_000, () => made.close());
    sessions.set(environment, made);
    return made;
  }

  return (registration, payload) => new Promise(resolve => {
    const environment = registration.environment === 'production' ? 'production' : 'sandbox';
    const request = session(environment).request({
      ':method': 'POST',
      ':path': `/3/device/${registration.token}`,
      authorization: `bearer ${bearer()}`,
      'apns-topic': registration.topic,
      'apns-push-type': 'alert',
      'apns-priority': '10',
      'content-type': 'application/json',
    });
    let status = 0;
    let body = '';
    request.setTimeout(15_000, () => request.close());
    request.on('response', headers => { status = Number(headers[':status']); });
    request.on('data', chunk => { body += chunk; });
    request.on('end', () => {
      let reason: string | undefined;
      try { reason = (JSON.parse(body) as { reason?: unknown }).reason as string | undefined; } catch {}
      if (status === 200) return resolve({ ok: true });
      resolve({
        ok: false,
        // 410: the app is gone from that phone.
        gone: status === 410,
        // 400 BadDeviceToken: a token this environment doesn't know, which may be the other's.
        wrongEnvironment: status === 400 && (reason === 'BadDeviceToken' || body.includes('BadDeviceToken')),
        reason: typeof reason === 'string' && /^[A-Za-z]{1,60}$/.test(reason) ? reason : status ? `HTTP ${status}` : 'no answer',
      });
    });
    request.on('error', () => resolve({ ok: false, reason: 'no answer' }));
    request.end(JSON.stringify(payload));
  });
}
