# The web UI: one page, and nothing fetched from anywhere else.
#
# A server that pulls a stylesheet from a CDN does not work on the
# closed network this is for, so there is no CDN, no framework and no
# image -- HTML, CSS and JavaScript in one document, served from
# memory (HTTP_API.md section 3). Embedding it also means the UI
# cannot 404 because an install put the file somewhere else.
#
# **The page is one raw literal** (`r#"..."#`): no escape is decoded
# and no `{...}` is interpolated, so what is below is the HTML exactly
# as it is served -- braces, backslashes and quotes as written, with no
# doubling to get wrong. The one thing it cannot contain is `"#`, which
# would end the literal; add a `#` to both ends if it ever needs one.
# (Attributes and the script still use single quotes. That is left
# over from when a `"` could not be written in a literal at all;
# changing it is a change to the page, not to how it is spelled.)
#
# It talks to `/v1/query?format=json` and nothing else. The state
# lives in the URL, so a result someone found is a link they can
# send.

# The page, appended to `out`.
pub fn page(out: &mut ByteWriter) {
    out.put_str(r#"<!doctype html>
<html lang='en'>
<head>
<meta charset='utf-8'>
<meta name='viewport' content='width=device-width, initial-scale=1'>
<title>logsearch</title>
<style>
:root { color-scheme: light dark; --line: #8883; --dim: #8888; }
* { box-sizing: border-box; }
body { margin: 0; font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
header { display: flex; gap: 8px; flex-wrap: wrap; align-items: center;
         padding: 10px 12px; border-bottom: 1px solid var(--line); }
input, select, button { font: inherit; padding: 4px 6px; }
input[name=q] { flex: 1 1 22em; min-width: 12em; }
input[name=limit] { width: 6em; }
main { padding: 0 12px; }
table { border-collapse: collapse; width: 100%; }
td { padding: 2px 8px 2px 0; vertical-align: top; border-bottom: 1px solid var(--line); }
td.ts { white-space: nowrap; color: var(--dim); }
td.body { word-break: break-all; }
footer { padding: 10px 12px; color: var(--dim); border-top: 1px solid var(--line); }
footer dl { display: grid; grid-template-columns: max-content auto; gap: 0 10px; margin: 0; }
dt { color: var(--dim); }
dd { margin: 0; }
.err { color: #c33; padding: 10px 12px; }
.note { color: var(--dim); padding: 10px 12px; }
.note code { color: inherit; }
.more { margin: 10px 0; }
</style>
</head>
<body>
<header>
  <form id='f'>
    <input name='q' placeholder='status=404 path~/wp- timeout' autofocus>
    <input name='limit' value='50' inputmode='numeric'>
    <button>search</button>
  </form>
</header>
<main>
  <div id='err' class='err' hidden></div>
  <div id='empty' class='note' hidden></div>
  <table><tbody id='rows'></tbody></table>
  <div class='more'><button id='more' hidden>more</button></div>
</main>
<footer><dl id='stats'></dl></footer>
<script>
const $ = (id) => document.getElementById(id);
const form = $('f'), rows = $('rows'), stats = $('stats'), err = $('err'), more = $('more');
const empty = $('empty');

// The state lives in the URL, so a result is a link someone can send.
function fromUrl() {
  const p = new URLSearchParams(location.search);
  form.q.value = p.get('q') || '';
  form.limit.value = p.get('limit') || '50';
}

function label(k) {
  return k.replace(/_/g, ' ');
}

// Nothing found and nothing to search are different things, and the
// difference is the first one a new reader hits: `serve` with no
// argument points at an empty archive, and an empty table alone looks
// like a query that missed.
function emptyState(data) {
  if (data.records.length > 0) { empty.hidden = true; return; }
  empty.hidden = false;
  if (data.stats.segments_considered === 0) {
    empty.textContent = 'This archive holds no segments. Fill one with: logsearch archive <logdir> <spec>';
  } else {
    empty.textContent = 'No records matched. `=` matches a whole value; try `~` for a part of one.';
  }
}

function show(data) {
  emptyState(data);
  rows.replaceChildren();
  for (const r of data.records) {
    const tr = document.createElement('tr');
    const ts = document.createElement('td');
    ts.className = 'ts';
    ts.textContent = r.ts === null ? '-' : r.ts;
    const body = document.createElement('td');
    body.className = 'body';
    body.textContent = r.body;
    tr.append(ts, body);
    rows.append(tr);
  }
  // Why it cost what it did, next to what it found. A search that
  // opened 4 of 12 segments is a different thing from one that opened
  // all 12, and the number is the only way to see which happened.
  stats.replaceChildren();
  for (const [k, v] of Object.entries(data.stats)) {
    const dt = document.createElement('dt');
    dt.textContent = label(k);
    const dd = document.createElement('dd');
    dd.textContent = String(v);
    stats.append(dt, dd);
  }
  // There are no cursors yet, so 'more' asks for a larger limit
  // rather than for the next page. It stops at the server's cap.
  const n = data.stats.shown;
  more.hidden = !(data.stats.truncated || n >= Number(form.limit.value));
}

async function run(push) {
  const q = form.q.value.trim();
  const limit = form.limit.value.trim() || '50';
  if (!q) { return; }
  const url = '/v1/query?format=json&q=' + encodeURIComponent(q) + '&limit=' + encodeURIComponent(limit);
  if (push) { history.pushState(null, '', '?q=' + encodeURIComponent(q) + '&limit=' + encodeURIComponent(limit)); }
  err.hidden = true;
  try {
    const res = await fetch(url);
    const data = await res.json();
    if (!res.ok) { throw new Error(data.detail || data.error || res.status); }
    show(data);
  } catch (e) {
    rows.replaceChildren();
    stats.replaceChildren();
    more.hidden = true;
    empty.hidden = true;
    err.textContent = String(e.message || e);
    err.hidden = false;
  }
}

form.addEventListener('submit', (e) => { e.preventDefault(); run(true); });
more.addEventListener('click', () => {
  form.limit.value = Math.min(1000, Number(form.limit.value) * 4);
  run(true);
});
window.addEventListener('popstate', () => { fromUrl(); run(false); });
fromUrl();
if (form.q.value) { run(false); }
</script>
</body>
</html>
"#)
}

# `text/html`, and the length is whatever the page came to.
pub fn content_type() -> str { "text/html; charset=utf-8" }
