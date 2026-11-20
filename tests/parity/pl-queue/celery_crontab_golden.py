import json, hashlib
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from celery import Celery
from celery.schedules import crontab, crontab_parser
app = Celery(set_as_current=True)
app.conf.timezone = "Europe/Zurich"
app.conf.enable_utc = True
tz = ZoneInfo("Europe/Zurich")
exprs = ["*/10 * * * *", "5 */1 * * *", "0 0 * * *", "30 0 * * sun", "0 1 * * *", "10 2 * * *", "0 2 * * *",
         "*/50 * * * mon", "0 12 1 * *", "15 3 29 2 *", "0 9 * * 1-5", "0 0 31 * *", "0 8 1-7 * mon", "45 23 * dec fri",
         "0,30 9-17 * * *", "7 * * * *", "0 22-2 * * *", "*/7 */5 * * *", "0 0 * * sat-mon", "3,6,9 1,2,3 * * *"]
lasts = ["2024-01-01T00:00:00", "2024-01-01T10:05:30", "2024-02-28T23:59:10", "2024-03-31T01:30:00",
         "2024-10-27T01:55:00", "2024-12-31T23:50:00", "2024-06-15T12:34:56", "2024-03-30T02:10:00"]
out = []
for e in exprs:
    m, h, dom, mon, dow = e.split(" ")
    for l in lasts:
        last = datetime.fromisoformat(l).replace(tzinfo=tz)
        now = last + timedelta(seconds=1)
        c = crontab(m, h, dow, dom, mon, app=app, nowfun=lambda now=now: now)
        rem = c.remaining_estimate(last)
        nxt = now.astimezone(ZoneInfo("UTC")) + rem
        out.append([e, last.isoformat(), nxt.isoformat()])
sets = {}
for spec, mx, mn in [("*/15",60,0),("1-5/2",60,0),("50-10",60,0),("mon",7,0),("mar",7,0),("sun-sat",7,0),("jan,mar",12,1),("*/3",31,1),("1-5-9",60,0),("*/5/2",60,0)]:
    sets[f"{spec}|{mx}|{mn}"] = sorted(crontab_parser(mx, mn).parse(spec))
errs = {}
for spec, mx, mn in [("7",7,0),("*/",60,0),("",60,0),("1,",60,0),("5/2",60,0),("xyz",60,0),("60",60,0),("0",31,1),("1-5/",60,0),("-1",60,0), ("*/0",60,0)]:
    try:
        crontab_parser(mx, mn).parse(spec); errs[f"{spec}|{mx}|{mn}"] = "ok"
    except Exception as ex:
        errs[f"{spec}|{mx}|{mn}"] = type(ex).__name__ + ": " + str(ex)
offs = {s: int.from_bytes(hashlib.sha256(s.encode()).digest()) % 10 for s in ["test-secret", "paperless-rs-reference-secret-key-local-only-0123456789", "x"]}
print(json.dumps({"next": out, "sets": sets, "errs": errs, "offsets": offs}))
