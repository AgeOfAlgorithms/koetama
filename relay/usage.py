"""The relay's usage per day, from Cloudflare's GraphQL analytics, and this month's against the Workers Paid plan.

    python relay/usage.py [DAYS]      (default 14; days are UTC, as Cloudflare resets its limits at 00:00 UTC)

Read-only. It uses wrangler's own login (the OAuth token in wrangler's config; never printed): if it says the token
is refused, run `npx wrangler whoami` from relay/ (with the `relay` conda env) to refresh it, or `npx wrangler login`.
The account is the first one the token can see (CF_ACCOUNT_ID picks another).

"DO req" is what the Durable Objects counted: every WebSocket message in, raw. The account is on Workers Paid ($5 a
month, since 2026-10-20): no daily limit; each month includes 1 million Durable Object requests (incoming WebSocket
messages billed 20:1) and 400,000 GB-s, then $0.15 a million and $12.50 a million GB-s. The daily columns still show
the old Free plan's 100,000 a day, for scale. A player talking sends ~17 packets a second (60 ms each).
"""
import datetime, json, os, re, sys, urllib.error, urllib.request

FREE_REQUESTS = 100_000      # a day, Durable Objects and Workers each
FREE_GBS = 13_000            # Durable Objects duration, GB-s a day
PAID_REQUESTS = 1_000_000    # a month, included in Workers Paid (incoming WebSocket messages 20:1)
PAID_GBS = 400_000           # a month
MEMORY_GB = 0.125            # a Durable Object is billed at 128 MB while active


def token():
    cfg = os.path.join(os.environ.get('APPDATA', os.path.expanduser('~')), 'xdg.config', '.wrangler', 'config', 'default.toml')
    if not os.path.exists(cfg):
        cfg = os.path.join(os.path.expanduser('~'), '.wrangler', 'config', 'default.toml')
    m = re.search(r'oauth_token = "([^"]+)"', open(cfg, encoding='utf-8').read())
    if not m:
        sys.exit('no wrangler login: run `npx wrangler login` from relay/')
    return m.group(1)


def call(url, body=None):
    req = urllib.request.Request(url, data=json.dumps(body).encode() if body else None,
                                 headers={'Authorization': 'Bearer ' + TOKEN, 'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        if e.code in (401, 403):
            sys.exit('Cloudflare refused the token: run `npx wrangler whoami` from relay/ to refresh it')
        raise


def main():
    days = int(sys.argv[1]) if len(sys.argv) > 1 else 14
    today = datetime.datetime.now(datetime.timezone.utc).date()
    days = max(days, today.day)  # (and the whole month so far)
    account = os.environ.get('CF_ACCOUNT_ID') or call('https://api.cloudflare.com/client/v4/accounts')['result'][0]['id']
    end = datetime.datetime.now(datetime.timezone.utc).date() + datetime.timedelta(days=1)
    v = {'a': account, 's': (end - datetime.timedelta(days=days)).isoformat(), 'e': end.isoformat()}
    q = '''query($a: String!, $s: Date!, $e: Date!) { viewer { accounts(filter: {accountTag: $a}) {
      workersInvocationsAdaptive(limit: 1000, filter: {date_geq: $s, date_lt: $e, scriptName: "koetama-relay"}) {
        dimensions { date } sum { requests errors } }
      durableObjectsInvocationsAdaptiveGroups(limit: 1000, filter: {date_geq: $s, date_lt: $e}) {
        dimensions { date } sum { requests } }
      durableObjectsPeriodicGroups(limit: 1000, filter: {date_geq: $s, date_lt: $e}) {
        dimensions { date } sum { activeTime outboundWebsocketMsgCount } } } } }'''
    r = call('https://api.cloudflare.com/client/v4/graphql', {'query': q, 'variables': v})
    if r.get('errors'):
        sys.exit('Cloudflare: ' + json.dumps(r['errors'])[:500])
    acc = r['data']['viewer']['accounts'][0]
    rows = {}
    for x in acc['workersInvocationsAdaptive']:
        d = rows.setdefault(x['dimensions']['date'], {})
        d['conn'] = d.get('conn', 0) + x['sum']['requests']
        d['err'] = d.get('err', 0) + x['sum']['errors']
    for x in acc['durableObjectsInvocationsAdaptiveGroups']:
        d = rows.setdefault(x['dimensions']['date'], {})
        d['req'] = d.get('req', 0) + x['sum']['requests']
    for x in acc['durableObjectsPeriodicGroups']:
        d = rows.setdefault(x['dimensions']['date'], {})
        d['active'] = d.get('active', 0) + x['sum']['activeTime'] / 1e6
        d['out'] = d.get('out', 0) + x['sum']['outboundWebsocketMsgCount']
    print('date (UTC)  connections errors   DO req  of 100k raw  of 100k 20:1   sent out  active s  of 13k GB-s')
    for day in sorted(rows):
        d = rows[day]
        req, active = d.get('req', 0), d.get('active', 0)
        print(f"{day}  {d.get('conn', 0):11} {d.get('err', 0):6} {req:8} {req / FREE_REQUESTS:11.1%} {req / 20 / FREE_REQUESTS:13.2%}"
              f" {d.get('out', 0):10} {active:9.0f} {active * MEMORY_GB / FREE_GBS:11.2%}")
    month = today.strftime('%Y-%m')
    mreq = sum(d.get('req', 0) for k, d in rows.items() if k.startswith(month))
    mgbs = sum(d.get('active', 0) for k, d in rows.items() if k.startswith(month)) * MEMORY_GB
    print(f"\n{month} so far (Workers Paid): about {mreq / 20:,.0f} billed Durable Object requests "
          f"({mreq / 20 / PAID_REQUESTS:.2%} of the 1 million included), {mgbs:,.0f} GB-s ({mgbs / PAID_GBS:.3%} of 400,000)")
    top = max(rows.values(), key=lambda d: d.get('req', 0), default={})
    print(f"\nbusiest day: {top.get('req', 0)} DO requests - at 17 packets a second, about "
          f"{top.get('req', 0) / 17 / 60:.0f} talking-minutes of one player")


if __name__ == '__main__':
    TOKEN = token()
    main()
