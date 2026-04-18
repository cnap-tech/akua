/**
 * akua playground entry point.
 *
 * The whole SDK is lazy-loaded: we don't init WASM until the user
 * clicks Pull. Keeps time-to-first-paint under 100ms. If the user
 * never clicks, the WASM module never downloads.
 */

import './style.css';

type PullResult = {
  name: string;
  version: string;
  description?: string;
  appVersion?: string;
  type?: string;
  kubeVersion?: string;
  dependencies?: Array<{ name: string; version: string; repository: string }>;
  maintainers?: Array<{ name?: string; email?: string; url?: string }>;
  home?: string;
  icon?: string;
  sources?: string[];
};

const form = must<HTMLFormElement>('pull-form');
const refInput = must<HTMLInputElement>('ref');
const pullBtn = must<HTMLButtonElement>('pull-btn');
const outputCard = must<HTMLElement>('output-card');
const output = must<HTMLDivElement>('output');
const errorCard = must<HTMLElement>('error-card');
const errorOut = must<HTMLPreElement>('error');

for (const example of document.querySelectorAll<HTMLButtonElement>('.example')) {
  example.addEventListener('click', () => {
    const ref = example.dataset['ref'];
    if (ref) {
      refInput.value = ref;
      refInput.focus();
    }
  });
}

form.addEventListener('submit', async (event) => {
  event.preventDefault();
  const ref = refInput.value.trim();
  if (!ref) return;

  pullBtn.disabled = true;
  pullBtn.textContent = 'Pulling…';
  errorCard.hidden = true;
  outputCard.hidden = true;

  const started = performance.now();
  try {
    // Lazy-load the SDK only when the user actually pulls. WASM
    // init (~200 ms on cold page) happens inside `init()`.
    const { init, pullChart, inspectChartBytes } = await import('@akua/sdk/browser');
    await init();

    const bytes = await pullChart(ref);
    const inspected = await inspectChartBytes(bytes);

    const elapsed = Math.round(performance.now() - started);
    renderChart(inspected.chartYaml as unknown as PullResult, bytes.byteLength, elapsed, ref);
  } catch (err) {
    renderError(err);
  } finally {
    pullBtn.disabled = false;
    pullBtn.textContent = 'Pull';
  }
});

function renderChart(chart: PullResult, tgzSize: number, elapsedMs: number, ref: string): void {
  output.innerHTML = '';

  const summary = el('div', 'summary');
  summary.append(
    kv('Name', chart.name ?? '(unknown)'),
    kv('Version', chart.version ?? '(unknown)'),
    kv('appVersion', chart.appVersion ?? '—'),
    kv('Type', chart.type ?? 'application'),
    kv('Tarball size', `${(tgzSize / 1024).toFixed(1)} KB`),
    kv('Pulled in', `${elapsedMs} ms`),
  );
  output.append(summary);

  if (chart.description) {
    output.append(block('Description', chart.description));
  }

  if (chart.maintainers?.length) {
    const names = chart.maintainers
      .map((m) => [m.name, m.email && `<${m.email}>`].filter(Boolean).join(' '))
      .join(', ');
    output.append(block('Maintainers', names));
  }

  if (chart.dependencies?.length) {
    const list = el('ul', 'deps');
    for (const dep of chart.dependencies) {
      const li = el('li');
      li.textContent = `${dep.name} @ ${dep.version} — ${dep.repository}`;
      list.append(li);
    }
    output.append(header('Dependencies'), list);
  }

  const source = el('details');
  const sum = el('summary');
  sum.textContent = 'Raw Chart.yaml';
  source.append(sum);
  const pre = el('pre');
  pre.textContent = JSON.stringify(chart, null, 2);
  source.append(pre);
  output.append(source);

  const trace = el('p', 'trace');
  trace.innerHTML = `<code>pullChart('${escape(ref)}')</code> → <code>inspectChartBytes(…)</code> — ${elapsedMs} ms, ${(tgzSize / 1024).toFixed(1)} KB over the wire. No proxy, no backend.`;
  output.append(trace);

  outputCard.hidden = false;
}

function renderError(err: unknown): void {
  const message = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
  errorOut.textContent = message;
  errorCard.hidden = false;
}

// ---------------------------------------------------------------------------
// DOM helpers — kept local; no framework.
// ---------------------------------------------------------------------------

function must<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing #${id}`);
  return node as T;
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}

function kv(key: string, value: string): HTMLElement {
  const row = el('div', 'kv');
  const k = el('span', 'k');
  k.textContent = key;
  const v = el('span', 'v');
  v.textContent = value;
  row.append(k, v);
  return row;
}

function header(text: string): HTMLElement {
  const h = el('h3');
  h.textContent = text;
  return h;
}

function block(title: string, body: string): HTMLElement {
  const wrap = el('div', 'block');
  wrap.append(header(title));
  const p = el('p');
  p.textContent = body;
  wrap.append(p);
  return wrap;
}

function escape(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}
