'use strict';

const $ = (id) => document.getElementById(id);
const state = { csrf: '', revision: '', systems: [], native: {}, locked: false, route: {}, epoch: 0, events: null, waiting: false, reconnect: null, jobs: new Map(), jobSignals: new Map(), webView: 'grid', browsers: [] };
function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === 'class') node.className = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (value !== null && value !== undefined && value !== false) node.setAttribute(key, value === true ? '' : String(value));
  }
  for (const child of children.flat(Infinity)) if (child !== null && child !== undefined) node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  return node;
}
function button(label, run, primary = false) {
  const node = el('button', { type: 'button', class: primary ? 'primary' : '' }, label);
  node.addEventListener('click', async () => {
    node.disabled = true;
    try { await run(); } catch (error) { showError(error); } finally { node.disabled = false; }
  });
  return node;
}
function field(label, input, help) { return el('label', {}, label, input, help ? el('small', {}, help) : null); }
function input(value = '', type = 'text') { const node = el('input', { type }); node.value = value; return node; }
function select(choices, value) {
  const node = el('select');
  for (const choice of choices) {
    const [id, label] = Array.isArray(choice) ? choice : [choice, choice];
    node.append(el('option', { value: id ?? '' }, label));
  }
  if (value !== undefined) node.value = value ?? '';
  return node;
}
function showError(error) {
  if(error.code==='superseded')return;
  const container = $('editor').open ? $('editor-body') : $('page-error');
  if (container === $('page-error')) { container.hidden = false; container.textContent = error.message || String(error); }
  else {
    container.querySelector('.error')?.remove();
    container.prepend(el('p', { class: 'notice error', role: 'alert' }, error.message || String(error)));
  }
}
function notice(message) { return el('p', { class: 'notice' }, message); }
function empty(message) { return el('div', { class: 'empty' }, el('p', {}, message)); }
function modal(title, ...nodes) {
  $('editor-title').textContent = title;
  $('editor-body').replaceChildren(...nodes.flat().filter(node => node != null));
  $('editor').dataset.revision = state.revision;
  if (!$('editor').open) $('editor').showModal();
  placeJobs();
}
function closeModal() { $('editor').close(); }
function placeJobs() {
  const target = $('editor').open ? $('editor-body') : $('jobs');
  for (const row of state.jobs.values()) target.prepend(row);
}
async function formModal(title, fields, save, label = 'Save changes', extra = []) {
  const form = el('form', {}, fields, el('div', { class: 'actions' }, el('button', { type: 'submit', class: 'primary' }, label), button('Cancel', closeModal)));
  form.addEventListener('submit', async (event) => {
    event.preventDefault(); const submit = form.querySelector('[type=submit]'); submit.disabled = true;
    try { await save(); } catch (error) { showError(error); } finally { submit.disabled = false; }
  });
  modal(title, form, ...extra);
}
function promptName(title, value, save, extra = []) {
  const name = input(value); name.required = true; name.maxLength = 255;
  return formModal(title, [field('Name', name), ...extra], () => save(name.value));
}
function image(path, cls = '') {
  if (!path) return el('div', { class: 'no-image ' + cls }, 'No artwork');
  const node = el('img', { src: path, alt: '', loading: 'lazy', width: 320, height: 240 });
  const frame = el('span', { class: 'web-image ' + cls }, node);
  node.addEventListener('error', () => frame.replaceWith(el('div', { class: 'no-image ' + cls }, 'Artwork could not be loaded')), { once: true });
  return frame;
}
async function request(path, body, method = 'POST') {
  let response;
  try {
    response = await fetch('/api/v1/' + path, { method, credentials: 'same-origin', headers: { 'Content-Type': 'application/json', 'X-Degauss-CSRF': state.csrf }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(20000) });
  } catch (cause) {
    offline();
    throw new Error('Degauss did not acknowledge the request. No command will be retried automatically. Check the connection and current state before retrying.', { cause });
  }
  let value;
  try { value = await response.json(); } catch { throw new Error('Degauss returned an unreadable response (' + response.status + ').'); }
  if (!response.ok) {
    if (response.status === 401) signedOut();
    const error = new Error(value.error?.message || 'Request failed (' + response.status + ').');
    error.code = value.error?.code; throw error;
  }
  if (value.revision) state.revision = value.revision;
  return value;
}
const command = (action, values = {}) => request('command', { action, ...values });
const edit = (action, values = {}) => command(action, { revision: $('editor').open ? $('editor').dataset.revision : state.revision, ...values });
const libraryReads = new Set(['browse', 'explore', 'game', 'history', 'scripts', 'random', 'home-artwork']);
async function operation(action, values = {}) {
  if (!libraryReads.has(action)) { const answer=await command(action,values); return answer.job?waitJob(answer.job):answer; }
  const previous=state.libraryRead;
  let release,started; const finished=new Promise(resolve=>{release=resolve;});
  const current={id:null,finished,started:new Promise(resolve=>{started=resolve;})}; state.libraryRead=current;
  try {
    if(previous){previous.replaced=true;await previous.started; if(previous.id!==null)await command('cancel',{id:previous.id}); await previous.finished;}
    if(current.replaced){const error=new Error('This read was replaced by the next navigation.');error.code='superseded';throw error;}
    let answer; try{answer=await command(action,values);current.id=answer.job??null;}finally{started();}
    return answer.job?await waitJob(answer.job, 'Reading'):answer;
  } catch(error) {if(current.replaced&&error.code==='cancelled')error.code='superseded';throw error;}
  finally {started();release();if(state.libraryRead===current)state.libraryRead=null;}
}
function progressText(message, verb = 'Working') { return message ? (/^(reading|loading|working)\b/i.test(message) ? message : verb + ': ' + message) : verb + '…'; }
function jobChanged(id) {
  const signal = state.jobSignals.get(id);
  if (signal) { signal.changed = true; signal.wake?.(); }
}
function pauseJob(signal) {
  if (signal.changed) return Promise.resolve();
  return new Promise(resolve => {
    const finish = () => { clearTimeout(timer); signal.wake = null; resolve(); };
    const timer = setTimeout(finish, 500); signal.wake = finish;
  });
}
async function waitJob(id, verb = 'Working') {
  const row = el('div', { class: 'job' }), label = el('p', {}, progressText('', verb));
  const cancel = button('Cancel', () => command('cancel', { id }));
  const signal = { changed: false, wake: null }; state.jobSignals.set(id, signal);
  row.append(label, cancel); state.jobs.set(id, row); placeJobs();
  try {
    for (;;) {
      signal.changed = false;
      const job = await command('job', { id });
      if (job.state === 'complete') { if (job.result?.revision) state.revision = job.result.revision; return job.result; }
      if (job.state === 'failed' || job.state === 'cancelled') {const error=new Error(job.error?.message || job.state);error.code=job.error?.code||job.state;throw error;}
      label.textContent = progressText(job.message, verb);
      cancel.hidden = job.cancellable === false;
      if (job.total) label.append(el('progress', { max: job.total, value: job.done || 0 }));
      await pauseJob(signal);
    }
  } finally { row.remove(); state.jobs.delete(id); state.jobSignals.delete(id); }
}
function signedOut() {
  state.csrf = ''; state.events?.close(); state.events = null;
  $('shell').hidden = true; $('login').hidden = false;
  $('transport').textContent = location.protocol === 'https:' ? 'Encrypted HTTPS connection.' : 'HTTP is unencrypted. Use only on a trusted local network.';
}
function applyState(value) {
  state.revision = value.revision || state.revision; state.native = value.native || state.native; state.locked = value.locked ?? state.locked;
  if (typeof value.crt_images === 'boolean') document.documentElement.classList.toggle('crt-images', value.crt_images);
  if (value.theme) for (const [key, color] of Object.entries(value.theme)) if (/^#[0-9a-f]{6}$/i.test(color)) document.documentElement.style.setProperty('--' + key.replaceAll('_', '-'), color);
  if (value.version) $('version').textContent = 'Degauss ' + value.version;
  if (value.font) {
    const fonts = { smooth: 'Degauss', 'smooth 2': 'DegaussHeading', pixel: 'DegaussPixel', 'pixel 2': 'DegaussPixel2' };
    if (fonts[value.font]) document.documentElement.style.setProperty('--interface-font', fonts[value.font]);
  }
  if (value.text_size) document.documentElement.style.setProperty('--interface-size', ({ smaller: '14px', small: '15px', default: '16px', large: '18px', larger: '20px' })[value.text_size] || '16px');
  const remote = $('native-state'); if (remote) remote.textContent = [state.native.screen, state.native.system, state.native.selected].filter(Boolean).join(' · ');
}
async function connected() {
  $('login').hidden = true; $('shell').hidden = false; $('connection').hidden = true;
  state.waiting = false;
  applyState(await command('state'));
  state.systems = (await command('systems')).items;
  state.events?.close(); state.events = new EventSource('/api/v1/events');
  state.events.onmessage = (event) => {
    const value = JSON.parse(event.data);
    if (value.type === 'state') applyState(value);
    if (value.type === 'job') jobChanged(value.job);
    if (value.type === 'resync') command('state').then(applyState).catch(showError);
  };
  state.events.onerror = offline;
  await renderRoute();
}
function offline() {
  if (!state.csrf) return;
  $('connection').hidden = false;
  $('connection').textContent = state.waiting ? 'Launched on MiSTer. Web Access is paused until Degauss returns. This page will reconnect.' : 'Connection to Degauss lost. It may be closed or the network may be unavailable. Edits are not retried automatically.';
  state.events?.close(); state.events = null;
  if (state.reconnect) return;
  state.reconnect = setTimeout(async () => {
    state.reconnect = null;
    try { const session = await request('session', undefined, 'GET'); state.csrf = session.csrf; await connected(); } catch { if (state.csrf) offline(); }
  }, 3000);
}
async function launch(id) {
  const response = await command('launch', { id });
  if (response.accepted) { closeModal(); state.waiting = true; offline(); }
}
function routeHref(page, data = {}) { return '#' + page + (Object.keys(data).length ? '/' + encodeURIComponent(JSON.stringify(data)) : ''); }
function go(page, data = {}) { location.hash = routeHref(page, data); }
function currentRoute() {
  const hash = location.hash.slice(1); const at = hash.indexOf('/');
  if (at < 0) return { page: hash || 'home' };
  return { ...JSON.parse(decodeURIComponent(hash.slice(at + 1))), page: hash.slice(0, at) };
}
function header(title, ...actions) { $('page-title').textContent = title; $('page-actions').replaceChildren(...actions.filter(Boolean)); document.title = title + ' · Degauss'; }
const pageNames = { home: 'Home', library: 'Library', browse: 'Library', explore: 'Explore Games', collections: 'Collections', layouts: 'Layouts Designer', themes: 'Themes', settings: 'Settings', remote: 'Remote Control', cores: 'Cores', updates: 'Core Updates', scripts: 'Scripts', history: 'Last Played' };
function breadcrumbs(items) {
  const trail = [{ name: 'Home', page: 'home' }, ...items];
  $('breadcrumb').replaceChildren(el('ol', {}, trail.map((item, index) => el('li', {}, index === trail.length - 1 ? el('span', { 'aria-current': 'page' }, item.name) : el('a', { href: routeHref(item.page, item.data) }, item.name)))));
}
function categorySystems(systems, category) { return systems.filter(system => system.category === category); }
function isFavorites(system) { return /^favou?rites$/i.test(system?.category || ''); }
function detailText(details) {
  if (typeof details === 'string') return details;
  if (!details) return 'No game information in the selected data source.';
  return Object.entries(details).filter(([, value]) => value != null && value !== '').map(([key, value]) => ['description', 'desc'].includes(key) ? value : key.replaceAll('_', ' ') + ': ' + value).join('\n\n');
}
function renderTiles(items, open, more, rows = false) {
  state.previewEntries=items.filter(item=>item.kind!=='folder').slice(0,8);
  if (!items.length) return empty('No entries match this selection. Change the search or filters, or choose another folder.');
  const container = el('div', { class: rows ? 'rows' : 'grid' });
  for (const item of items) {
    const tile = el('article', { class: rows ? 'row' : 'tile' });
    const caption = el('span', { class: rows ? 'grow' : 'caption' }, el('span', {}, item.favorite ? '♥ ' : '', item.name), el('small', {}, [item.system_name || item.category, item.genre, item.games != null ? item.games + ' games' : null].filter(Boolean).join(' · ')));
    const primary = button('', () => open(item)); primary.className = 'open';
    primary.append(rows ? caption : [image(item.image), caption].reduce((fragment, node) => { fragment.append(node); return fragment; }, document.createDocumentFragment()));
    if (rows && item.image) tile.append(image(item.image));
    tile.append(primary);
    if (more) tile.append(el('div', { class: 'actions' }, button('Actions', () => more(item))));
    container.append(tile);
  }
  return container;
}
// The browser renders the current page only. Native layout assignments are
// deliberately outside this path; selected-game descriptions load on demand.
function renderBrowser(items, open, more, { page = null, move = null, games = false, artworkPending = false } = {}) {
  const root = el('section', { class: 'browser-view', 'aria-label': 'Library entries' });
  let selected = (page?.focus ?? state.route.focus) === 'last' ? items.length - 1 : 0, generation = 0;
  const details = new Map();
  const game = item => item.destination?.page === 'game' ? { ...item, ...item.destination } : games && item.kind !== 'folder' ? item : null;
  const previous = () => selected > 0 || Boolean(page?.offset);
  const next = () => selected + 1 < items.length || Boolean(page && page.offset + items.length < page.total);
  const step = delta => {
    if (selected + delta < 0 || selected + delta >= items.length) {
      if (page && move && (delta < 0 ? previous() : next())) move(delta < 0 ? Math.max(0, page.offset - page.limit) : page.offset + items.length, delta < 0 ? 'last' : 0);
      return;
    }
    selected += delta; paint();
  };
  const controls = item => {
    const target = game(item);
    return el('div', { class: 'actions' }, button(target ? 'Launch' : 'Open', () => target ? launch(target.id) : open(item), true), more ? button('Actions', () => more(item)) : null);
  };
  const paint = () => {
    const token = ++generation;
    const oldScroll = root.querySelector('.browser-details > .rows')?.scrollTop || 0;
    root.replaceChildren();
    if (!items.length) { root.append(empty('No entries match this selection. Change the search or filters, or choose another folder.')); return; }
    if (state.webView === 'grid' || state.webView === 'list') { root.append(renderTiles(items, open, more, state.webView === 'list')); return; }
    const item = items[selected], target = game(item);
    const before = button('Previous', () => step(-1)), after = button('Next', () => step(1));
    before.disabled = !previous(); after.disabled = !next();
    const navigation = el('div', { class: 'selection-navigation' }, before, el('span', { role: 'status' }, ((page?.offset || 0) + selected + 1).toLocaleString() + ' / ' + (page?.total || items.length).toLocaleString()), after);
    const panel = el('div', { class: 'selected-entry stack', tabindex: '0', 'aria-label': 'Selected entry. Left and right change selection.' }, image(item.image, 'detail-art'), el('h2', {}, item.name), el('p', { class: 'muted' }, [item.system_name || item.category, item.genre].filter(Boolean).join(' · ')), controls(item));
    panel.addEventListener('keydown', event => {
      if (event.target.matches('input,select,textarea') || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
      event.preventDefault(); step(event.key === 'ArrowLeft' ? -1 : 1);
      root.querySelector('.selected-entry')?.focus({ preventScroll: true });
    });
    if (state.webView === 'details') {
      const rows = renderTiles(items, entry => { selected = items.indexOf(entry); paint(); }, null, true);
      rows.children[selected]?.classList.add('selected');
      root.append(el('div', { class: 'browser-details' }, rows, panel), navigation);
      rows.scrollTop = oldScroll;
      const row = rows.children[selected];
      if (row && rows.clientHeight) {
        const edge = row.getBoundingClientRect(), list = rows.getBoundingClientRect();
        if (edge.top < list.top) rows.scrollTop += edge.top - list.top;
        else if (edge.bottom > list.bottom) rows.scrollTop += edge.bottom - list.bottom;
      }
    } else if (state.webView === 'gallery') {
      root.append(el('div', { class: 'browser-gallery' }, panel), navigation);
    } else {
      const neighbour = delta => {
        const entry = items[selected + delta];
        if (!entry) return el('div', { class: 'carousel-neighbour' });
        const node = button('', () => step(delta)); node.className = 'carousel-neighbour';
        node.setAttribute('aria-label', (delta < 0 ? 'Previous: ' : 'Next: ') + entry.name);
        node.append(image(entry.image), el('span', {}, entry.name)); return node;
      };
      panel.append(button('Details', () => open(item)));
      root.append(el('div', { class: 'browser-carousel' }, neighbour(-1), panel, neighbour(1)), navigation);
      return;
    }
    const information = el('div', { class: 'selected-information' }); panel.append(information);
    const show = data => information.replaceChildren(notices(data), el('pre', {}, detailText(data.details)));
    if (!target) { show(item); return; }
    if (details.has(target.id)) { show(details.get(target.id)); return; }
    information.append(el('p', { role: 'status' }, 'Reading full game information…'), el('pre', {}, detailText(item.details)));
    queueMicrotask(async () => {
      if (!root.isConnected || generation !== token || artworkPending) return;
      try {
        const data = await operation('game', { id: target.id });
        if (!root.isConnected || generation !== token) return;
        details.set(target.id, data); show(data);
        if (data.image !== item.image) panel.firstChild.replaceWith(image(data.image, 'detail-art'));
      } catch (error) {
        if (root.isConnected && generation === token && error.code !== 'superseded') information.replaceChildren(el('p', { class: 'notice error', role: 'alert' }, error.message));
      }
    });
  };
  root.refreshArtwork = images => { for (const item of items) if (images[item.key]) item.image = images[item.key]; artworkPending = false; paint(); };
  state.browsers = state.browsers.filter(browser => browser.root.isConnected);
  state.browsers.push({ root, paint }); paint(); return root;
}
function setWebView(value) {
  if (!['grid', 'list', 'details', 'gallery', 'carousel'].includes(value)) throw new Error('Unknown browser view.');
  state.webView = value;
  state.browsers = state.browsers.filter(browser => browser.root.isConnected);
  for (const browser of state.browsers) browser.paint();
}
function paginate(data, move) {
  const before = button('Previous', () => move(Math.max(0, data.offset - data.limit))); before.disabled = data.offset === 0;
  const after = button('Next', () => move(data.offset + data.limit)); after.disabled = data.offset + data.limit >= data.total;
  const pages = Math.ceil(data.total / data.limit);
  const page = select([['', 'Jump to page'], ...Array.from({ length: pages }, (_, index) => [index * data.limit, String(index + 1)])], data.total && data.offset % data.limit === 0 ? data.offset : '');
  page.disabled = pages < 2;
  page.onchange = () => { if (page.value !== '') move(Number(page.value)); };
  const letter = select([['', 'Jump to letter'], ...(data.letters || []).map(entry => [entry.offset, entry.letter])], '');
  letter.disabled = !data.letters?.length;
  letter.onchange = () => { if (letter.value !== '') move(Number(letter.value)); };
  const navigation = el('div', { class: 'pagination-navigation' }, before, el('span', {}, data.total ? (data.offset + 1).toLocaleString() + '–' + Math.min(data.offset + data.limit, data.total).toLocaleString() + ' / ' + data.total.toLocaleString() : '0 entries'), after);
  const jumps = el('div', { class: 'pagination-jumps' }, field('Page', page), field('Letter', letter));
  return el('div', { class: 'pagination' }, navigation, jumps);
}
function letterOffsets(items) {
  const letters = new Map();
  items.forEach((item, offset) => {
    const first = Array.from(item.name)[0] || ' ';
    const letter = /[\p{L}\p{N}]/u.test(first) ? first.toUpperCase() : '#';
    if (!letters.has(letter)) letters.set(letter, offset);
  });
  return Array.from(letters).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([letter, offset]) => ({ letter, offset }));
}
function notices(data) {
  if (!data.notices?.length && !data.warnings?.length) return document.createDocumentFragment();
  return el('details', {}, el('summary', {}, 'Library notices'), el('pre', {}, [...(data.notices || []), ...(data.warnings || [])].map(x => typeof x === 'string' ? x : JSON.stringify(x)).join('\n')));
}
async function openHomeItem(item) {
  const dest = item.destination;
  if (dest.page === 'launch') return launch(dest.id);
  if (dest.page === 'game') return gameDetails({ ...item, id: dest.id, system: dest.system });
  if (dest.page === 'category') {
    const special = { 'Explore Games': 'explore', 'Last Played': 'history', 'Core Updates': 'updates', 'Cores': 'cores', 'Scripts': 'scripts' };
    if (dest.category === 'Attract Mode') return command('attract');
    return go(special[dest.category] || 'library', { category: dest.category });
  }
  go(dest.page, dest);
}
async function homePage(route, epoch) {
  const data = await command('home', { folder: route.folder || null }); if (epoch !== state.epoch) return;
  state.revision = data.revision;
  const editMode = Boolean(route.edit);
  breadcrumbs(data.breadcrumbs.map(item => ({ name: item.name, page: 'home', data: { folder: item.folder } })));
  header(data.breadcrumbs.at(-1)?.name || 'Home', route.folder ? button('Up a level', () => go('home', { folder: data.parent })) : null, button(editMode ? 'Finish editing' : 'Edit ' + (route.folder ? 'Home Folder' : 'Home'), () => go('home', { ...route, edit: !editMode })), button('New folder', () => promptName('New personal folder', '', async name => { await edit('home-edit', { edit: { operation: 'folder', name, parent: route.folder || null } }); closeModal(); await renderRoute(); })));
  if (!route.folder) $('page-actions').append(button('Change MiSTer layout', () => chooseView('home')));
  if (!editMode) {
    const items=data.items.filter(x => !x.hidden && x.available !== false);
    const artworkPending = items.some(item=>item.destination.page==='game');
    const grid=renderBrowser(items,openHomeItem,item=>homeItemEditor(item,route), { artworkPending }); $('page').append(grid);
    if (artworkPending) {
      const art=await operation('home-artwork',{folder:route.folder||null}); if(epoch!==state.epoch)return;
      grid.refreshArtwork(art.images);
      $('page').append(notices(art));
    }
    return;
  }
  const rows = el('div', { class: 'rows' }); let dragging = null;
  const reorder = async (from, to) => {
    const keys = data.items.map(x => x.key), [moved] = keys.splice(from, 1); keys.splice(to, 0, moved);
    await edit('home-edit', { revision: data.revision, edit: { operation: 'reorder', folder: route.folder || null, keys } }); await renderRoute();
  };
  data.items.forEach((item, index) => {
    const up = button('↑', () => reorder(index, index - 1)); up.disabled = index === 0; up.setAttribute('aria-label', 'Move ' + item.name + ' up');
    const down = button('↓', () => reorder(index, index + 1)); down.disabled = index === data.items.length - 1; down.setAttribute('aria-label', 'Move ' + item.name + ' down');
    const row = el('div', { class: 'row' + (item.hidden ? ' dim' : ''), draggable: 'true', tabindex: '0' }, el('div', { class: 'grow' }, item.name, el('small', {}, item.hidden ? 'Hidden' : 'Visible')), el('div', { class: 'actions' }, up, down, button(item.hidden ? 'Show' : 'Hide', async () => { await edit('home-edit', { edit: { operation: 'visibility', key: item.key, visible: item.hidden } }); await renderRoute(); }), button('Edit', () => homeItemEditor(item, route))));
    row.addEventListener('dragstart', event => { dragging = index; event.dataTransfer.setData('text/plain', item.key); });
    row.addEventListener('dragover', event => { if (dragging !== null) { event.preventDefault(); row.classList.add('drop-target'); } });
    row.addEventListener('dragleave', () => row.classList.remove('drop-target'));
    row.addEventListener('dragend', () => { dragging = null; rows.querySelectorAll('.drop-target').forEach(n => n.classList.remove('drop-target')); });
    row.addEventListener('drop', event => { event.preventDefault(); if (dragging !== null && dragging !== index) reorder(dragging, index).catch(showError); dragging = null; });
    rows.append(row);
  });
  $('page').append(el('p', { class: 'muted' }, 'Drag entries to reorder, or use the arrow buttons. Changes are saved immediately.'), rows);
}
async function personalFolders() {
  const result = [[null, 'Home']];
  async function visit(folder, prefix) {
    const data = await command('home', { folder });
    for (const item of data.items) if (item.folder) { result.push([item.folder, prefix + item.name]); await visit(item.folder, prefix + item.name + ' / '); }
  }
  await visit(null, ''); return result;
}
async function addHome(item) {
  const parent = select(await personalFolders()), name = input(item.name);
  await formModal('Add to Home', [field('Name', name), field('Destination', parent)], async () => {
    await edit('home-edit', { edit: { operation: 'add', target: item.target || item.id, name: name.value, parent: parent.value || null } }); closeModal(); if (state.route.page === 'home') await renderRoute();
  }, 'Add');
}
async function homeItemEditor(item, route) {
  const id = item.key.startsWith('entry:') ? item.key.slice(6) : null;
  const actions = [button('Open', () => { closeModal(); return openHomeItem(item); }), button('Choose image', () => chooseImage(item.key.startsWith('entry:') ? item.key : item.target))];
  if (item.view_editable) actions.push(button('Change MiSTer layout', () => chooseView(item.target)));
  if (id) actions.push(
    button('Rename', () => promptName('Rename entry', item.name, async name => { await edit('home-edit', { edit: { operation: 'rename', id, name } }); closeModal(); await renderRoute(); })),
    button('Move to folder', async () => { const parent = select((await personalFolders()).filter(([folder]) => folder !== id)); await formModal('Move ' + item.name, [field('Destination', parent)], async () => { await edit('home-edit', { edit: { operation: 'move', id, parent: parent.value || null } }); closeModal(); await renderRoute(); }, 'Move'); }),
    button('Remove from Home', async () => { await edit('home-edit', { edit: { operation: 'remove', id } }); closeModal(); await renderRoute(); })
  );
  modal(item.name, el('div', { class: 'actions' }, actions), notice('Home entries are references. Removing one does not remove game files.'));
}
async function libraryPage(route, epoch) {
  const systems = (await command('systems')).items; if (epoch !== state.epoch) return; state.systems = systems;
  const selected = categorySystems(systems, route.category);
  if (route.category && selected.length === 1) return browsePage({ system: selected[0].system }, epoch);
  const category = select([['', 'All categories'], ...Array.from(new Set(systems.map(s => s.category)))], route.category || '');
  const search = input('', 'search'), result = el('div');
  const draw = () => { const filtered = systems.filter(s => (!category.value || s.category === category.value) && s.name.toLowerCase().includes(search.value.toLowerCase())); result.replaceChildren(renderBrowser(filtered, item => go('browse', { system: item.system }), systemActions)); };
  category.onchange = () => go('library', category.value ? { category: category.value } : {}); search.oninput = draw;
  breadcrumbs([{ name: 'Library', page: 'library' }, ...(route.category ? [{ name: route.category, page: 'library', data: { category: route.category } }] : [])]);
  header(route.category || 'Library');
  $('page').append(el('div', { class: 'toolbar' }, field('Find a system', search), field('Category', category)), result); draw();
}
async function browsePage(route, epoch) {
  const data = await operation('browse', { system: route.system, folder: route.folder || null, offset: route.offset || 0, limit: 60, search: route.search || '', filters: route.filters || {} });
  if (epoch !== state.epoch) return; state.revision = data.revision;
  const sys = state.systems.find(s => s.system === route.system);
  const category = sys && categorySystems(state.systems, sys.category).length > 1 ? [{ name: sys.category, page: 'library', data: { category: sys.category } }] : [];
  breadcrumbs([{ name: 'Library', page: 'library' }, ...category, ...data.breadcrumbs.map(item => ({ name: item.name, page: 'browse', data: { system: route.system, folder: item.folder } }))]);
  const name = data.breadcrumbs.at(-1)?.name || data.name;
  const current = { id: data.folder, target: data.folder, system: route.system, name, kind: 'folder' };
  header(name, randomMenu({ folder: data.folder, query: { ...route.filters, title: route.search || '' } }), data.parent ? button('Up a level', () => go('browse', { system: route.system, folder: data.parent })) : button('Systems', () => go('library', category.length ? { category: sys.category } : {})), button('Folder actions', () => folderActions(current, sys)), isFavorites(sys) ? button('Manage favourites folders', favoriteFolders) : null);
  const search = input(route.search || '', 'search'), form = el('form', { class: 'toolbar' }, field('Search this folder', search), el('button', { type: 'submit' }, 'Search'), button('Clear', () => go('browse', { ...route, search: '', offset: 0 })));
  form.onsubmit = e => { e.preventDefault(); go('browse', { ...route, search: search.value, offset: 0 }); };
  const move = (offset, focus = 0) => go('browse', { ...route, offset, focus });
  $('page').append(form, filterPanel(data.facets, route.filters || {}, query => go('browse', { ...route, filters: query, offset: 0 })), notices(data), renderBrowser(data.items, item => item.kind === 'folder' ? go('browse', { system: route.system, folder: item.id }) : gameDetails(item), item => item.kind === 'folder' ? folderActions(item, sys) : gameActions(item), { page: data, move, games: true }), paginate(data, move));
}
function filterPanel(facets, query, changed) {
  const fields = el('div', { class: 'filters' });
  for (const [name, choices] of Object.entries(facets || {})) {
    const current = name === 'Category' ? query.category : name === 'System' ? query.system : name === 'Decade' ? query.decade : query.fields?.[name];
    const dropdown = select(choices.map((choice, i) => [i, choice.label + (choice.count != null ? ' (' + choice.count + ')' : '')]), Math.max(0, choices.findIndex(choice => name === 'Category' || name === 'System' ? (choice.criterion?.Known?.label ?? null) === (current ?? null) : JSON.stringify(choice.criterion) === JSON.stringify(current ?? null))));
    dropdown.onchange = () => {
      const next = structuredClone(query), value = choices[Number(dropdown.value)].criterion;
      if (name === 'Category' || name === 'System') next[name.toLowerCase()] = value?.Known?.label ?? null;
      else if (name === 'Decade') next.decade = value;
      else { next.fields ||= {}; if (value == null) delete next.fields[name]; else next.fields[name] = value; }
      changed(next);
    };
    fields.append(field(name, dropdown));
  }
  return el('details', {}, el('summary', {}, 'Filters'), fields, button('Clear filters', () => changed({ title: query.title || '' })));
}
async function explorePage(route, epoch) {
  let query = route.query || {}, collection = null;
  if (route.collection) { const collections = await command('collections'); collection = collections.items[route.collection]; if (!collection) throw new Error('This collection no longer exists.'); query = route.query || collection.query; }
  const data = await operation('explore', { query, offset: route.offset || 0, limit: 60 }); if (epoch !== state.epoch) return;
  state.revision = data.revision;
  const search = input(query.title || '', 'search'), searchForm = el('form', { class: 'toolbar' }, field('Search all games', search), el('button', { type: 'submit' }, 'Search'));
  searchForm.onsubmit = event => { event.preventDefault(); go('explore', { ...route, query: { ...query, title: search.value }, offset: 0 }); };
  header(collection?.name || 'Explore Games', button('Save collection', () => promptName('Save collection', collection?.name || '', async name => { const saved = await edit('collection-save', { id: route.collection || null, name, query }); closeModal(); go('explore', { collection: saved.id }); })));
  if (collection) breadcrumbs([{ name: 'Collections', page: 'collections' }, { name: collection.name, page: 'explore', data: { collection: route.collection } }]);
  $('page-actions').append(randomMenu({ query }));
  $('page-actions').append(button('Change MiSTer layout', () => chooseView('explore')));
  const grouping = select([['', 'Game list'], ...Object.keys(data.facets).map(name => [name, 'Group by ' + name])], route.group || '');
  grouping.onchange = () => go('explore', { ...route, query, group: grouping.value, offset: 0 });
  $('page').append(field('Explore by', grouping));
  if (route.group) {
    const choices = (data.facets[route.group] || []).filter(choice => choice.criterion != null && choice.count > 0);
    $('page').append(searchForm, filterPanel(data.facets, query, next => go('explore', { ...route, query: next, offset: 0 })), notices(data), el('div', { class: 'rows' }, choices.map(choice => {
      return button(choice.label + ' (' + choice.count + ')', () => {
        const next = structuredClone(query), name = route.group;
        if (name === 'Category' || name === 'System') next[name.toLowerCase()] = choice.criterion.Known.label;
        else if (name === 'Decade') next.decade = choice.criterion;
        else { next.fields ||= {}; next.fields[name] = choice.criterion; }
        go('explore', { ...route, query: next, group: '', offset: 0 });
      });
    })));
    return;
  }
  const move = (offset, focus = 0) => go('explore', { ...route, query, offset, focus });
  $('page').append(searchForm, filterPanel(data.facets, query, next => go('explore', { ...route, query: next, offset: 0 })), notices(data), renderBrowser(data.items, gameDetails, gameActions, { page: data, move, games: true }), paginate(data, move));
}
async function collectionsPage(epoch) {
  const data = await command('collections'); if (epoch !== state.epoch) return;
  header('Collections', button('Explore games to create or edit', () => go('explore')));
  const items = Object.entries(data.items).map(([id, value]) => ({ ...value, id, target: data.targets[id] }));
  $('page').append(items.length ? renderBrowser(items, item => go('explore', { collection: item.id }), item => modal(item.name, el('div', { class: 'actions' }, button('Open collection', () => { closeModal(); go('explore', { collection: item.id }); }), button('Add to Home', () => addHome(item)), button('Rename', () => promptName('Rename collection', item.name, async name => { await edit('collection-save', { id: item.id, name, query: item.query }); closeModal(); await renderRoute(); })), button('Delete collection', async () => { await edit('collection-delete', { id: item.id }); closeModal(); await renderRoute(); })))) : empty('Save a filtered selection in Explore Games to create a collection.'));
}
async function gameDetails(item) {
  const artwork = image(item.image, 'detail-art');
  const actions = data => el('div', { class: 'actions' }, button('Launch', () => launch(item.id), true), typeof data.favorite === 'boolean' ? button('Actions', () => gameActions({ ...item, ...data })) : null);
  const content = el('div', { class: 'detail', 'aria-busy': 'true' }, artwork, el('div', { class: 'stack' }, el('p', { role: 'status' }, 'Reading full game information…'), el('pre', {}, detailText(item.details)), actions(item)));
  modal(item.name, content);
  try {
    const data = await operation('game', { id: item.id }); state.revision = data.revision;
    if (!$('editor').open || !$('editor-body').contains(content)) return;
    modal(data.name, notices(data), el('div', { class: 'detail' }, data.image === item.image ? artwork : image(data.image, 'detail-art'), el('div', { class: 'stack' }, el('pre', {}, detailText(data.details)), actions(data))));
  } finally { content.removeAttribute('aria-busy'); }
}
async function gameActions(item) {
  modal(item.name, el('div', { class: 'actions' }, button('Launch', () => launch(item.id), true), button('Game information', () => gameDetails(item)), button('Add to Home', () => addHome(item)), button(item.favorite ? 'Remove favourite' : 'Add favourite', async () => { if (item.favorite) { await command('favorite-remove', { game: item.id }); closeModal(); await renderRoute(); } else await addFavorite(item); }), button('Core choice', () => coreChoices(item.system, item.id)), button('Artwork', () => artworkActions(item)), button('Hide game', async () => { await edit('hide', { target: item.id, hidden: true }); closeModal(); await renderRoute(); })));
}
async function systemActions(item) {
  modal(item.name, el('div', { class: 'actions' }, button('Open', () => { closeModal(); go('browse', { system: item.system }); }), button('Add to Home', () => addHome(item)), button('Game data source', () => gameData(item.system)), button('Core choice', () => coreChoices(item.system)), button('Choose image', () => chooseImage(item.id)), button('Change MiSTer layout', () => chooseView(item.id)), button('Folder options', () => folderOptions(item.id)), button('Rebuild system', async () => { closeModal(); await operation('rebuild', { system: item.system }); await renderRoute(); }), button('Scrape system', () => scrapeConfirm(item.id, item.name)), button('Hide system', async () => { await edit('hide', { target: item.id, hidden: true }); closeModal(); await renderRoute(); })));
}
async function folderActions(item, system) {
  modal(item.name, el('div', { class: 'actions' }, button('Add to Home', () => addHome(item)), button('Folder options', () => folderOptions(item.id)), button('Change MiSTer layout', () => chooseView(item.id)), button('Scrape folder', () => scrapeConfirm(item.id, item.name)), system ? button('System actions', () => systemActions(system)) : null));
}
async function renderRoute() {
  if (!state.csrf) return; const epoch = ++state.epoch;
  state.browsers = [];
  $('page-error').hidden = true; $('page').replaceChildren(); $('breadcrumb').textContent = ''; $('page-actions').replaceChildren();
  try {
    const route = currentRoute(); state.route = route; $('navigation').classList.remove('open'); $('nav-toggle').setAttribute('aria-expanded', 'false');
    $('web-view-control').hidden = !['home','library','browse','explore','collections','history','updates','cores','scripts'].includes(route.page) || Boolean(route.edit);
    const name = pageNames[route.page] || 'Page';
    header(name); breadcrumbs(route.page === 'home' ? [] : [{ name, page: route.page }]);
    $('page-loading').textContent = 'Loading ' + name + '…'; $('page-loading').hidden = false; $('page').setAttribute('aria-busy', 'true');
    document.querySelectorAll('nav a').forEach(a => { if (a.hash.slice(1) === route.page) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current'); });
    const pages = { home: () => homePage(route, epoch), library: () => libraryPage(route, epoch), browse: () => browsePage(route, epoch), explore: () => explorePage(route, epoch), collections: () => collectionsPage(epoch), layouts: () => layoutsPage(epoch), themes: () => themesPage(epoch), settings: () => settingsPage(epoch), remote: remotePage, cores: () => coresPage(route, epoch), updates: () => updatesPage(route, epoch), scripts: () => scriptsPage(route, epoch), history: () => historyPage(epoch) };
    if (!pages[route.page]) throw new Error('Unknown page. Choose a page from the navigation.');
    await pages[route.page]();
  } catch (error) { if (epoch === state.epoch) showError(error); }
  finally { if (epoch === state.epoch) { $('page-loading').hidden = true; $('page').removeAttribute('aria-busy'); } }
}
document.querySelectorAll('.home-link').forEach(link => link.addEventListener('click', closeModal));
$('web-view').onchange = event => { try { setWebView(event.target.value); } catch (error) { showError(error); } };
$('nav-toggle').onclick = () => { const open = $('navigation').classList.toggle('open'); $('nav-toggle').setAttribute('aria-expanded', String(open)); };
$('logout').onclick = async () => { try { await request('logout', {}); signedOut(); } catch (error) { showError(error); } };
$('login-form').onsubmit = async event => {
  event.preventDefault(); const submit = event.target.querySelector('button'); submit.disabled = true; $('login-error').textContent = '';
  try { const result = await request('login', { password: $('password').value, remember: $('remember').checked }); $('password').value = ''; state.csrf = result.csrf; await connected(); }
  catch (error) { $('login-error').textContent = error.message; } finally { submit.disabled = false; }
};
window.addEventListener('hashchange', renderRoute);
$('editor').addEventListener('click', event => { if (event.target === $('editor')) { const r = $('editor').getBoundingClientRect(); if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) closeModal(); } });
$('editor').addEventListener('close', placeJobs);
request('session', undefined, 'GET').then(session => { state.csrf = session.csrf; return connected(); }).catch(error => { signedOut(); if (error.code !== 'authentication') $('login-error').textContent = error.message; });

async function historyPage(epoch) {
  const data = await operation('history'); if (epoch !== state.epoch) return;
  header('Last Played', button('Change MiSTer layout', () => chooseView('history')));
  $('page').append(notices(data), renderBrowser(data.items, gameDetails, gameActions, { games: true }));
}
async function scriptsPage(route, epoch) {
  const data = await operation('scripts', { folder: route.folder || null }); if (epoch !== state.epoch) return;
  breadcrumbs(data.breadcrumbs.map(item => ({ name: item.name, page: 'scripts', data: item.folder ? { folder: item.folder } : {} })));
  const parent = data.breadcrumbs.at(-2);
  header(data.breadcrumbs.at(-1).name, parent ? button('Up a level', () => go('scripts', parent.folder ? { folder: parent.folder } : {})) : null);
  $('page').append(notice('Launching a script pauses Web Access. Use the MiSTer display for interactive scripts.'),
    renderBrowser(data.items, item => item.script_folder ? go('scripts', { folder: item.script_folder }) : launch(item.id), item => item.id ? addHome(item) : Promise.resolve()));
}
async function coresPage(route, epoch) {
  const data = await command('cores', { category: route.category || null }); if (epoch !== state.epoch) return;
  header('Cores', button('Change MiSTer layout', () => chooseView(data.view_target))); const category = select([['', 'All categories'], ...data.categories], route.category || '');
  if (route.category) breadcrumbs([{ name: 'Cores', page: 'cores' }, { name: route.category, page: 'cores', data: { category: route.category } }]);
  category.onchange = () => go('cores', { category: category.value || null });
  $('page').append(field('Category', category), renderBrowser(data.items, item => launch(item.id), addHome));
}
async function updatesPage(route, epoch) {
  const data = await operation('core-updates', { refresh: Boolean(route.refresh), begin: !route.refresh }); if (epoch !== state.epoch) return;
  header('Core Updates', button('Refresh sources', () => go('updates', { refresh: true, run: Date.now() })));
  if (!data.loaded) { $('page').append(empty('Refresh Core Updates to read your configured download sources.')); return; }
  const filter = select([['all', 'All cores'], ['changed', "What's New"], ['updates', 'Updates available'], ['installed', 'Installed'], ['new', 'Not installed']]);
  const sort = select([['latest', 'Latest build'], ['name', 'Name']]), search = input('', 'search'), results = el('div');
  let offset = 0, focus = 0;
  const draw = () => {
    const items = data.items.filter(item => item.name.toLowerCase().includes(search.value.toLowerCase()) && (filter.value === 'all' || (filter.value === 'changed' && item.new) || (filter.value === 'updates' && item.update) || (filter.value === 'installed' && item.installed) || (filter.value === 'new' && !item.installed)));
    items.sort((a, b) => sort.value === 'name' ? a.name.localeCompare(b.name) : String(b.build || '').localeCompare(String(a.build || '')) || a.name.localeCompare(b.name));
    const page = { total: items.length, offset, limit: 60, letters: letterOffsets(items), focus };
    const move = (next, selection = 0) => { offset = next; focus = selection; draw(); results.scrollIntoView({ block: 'start' }); };
    results.replaceChildren(renderBrowser(items.slice(offset, offset + 60).map(item => ({ ...item, category: [item.source, item.state, item.build].filter(Boolean).join(' · ') })), coreUpdateDetails, null, { page, move }), paginate(page, move));
  };
  [search, filter, sort].forEach(node => node.addEventListener(node === search ? 'input' : 'change', () => { offset = 0; focus = 0; draw(); }));
  $('page').append(el('div', { class: 'toolbar' }, field('Find a core', search), field('Show', filter), field('Sort', sort)), data.notice ? notice(data.notice) : document.createDocumentFragment(), notice(data.history), results); draw();
  await command('core-updates-seen', { snapshot: data.snapshot });
}
async function coreUpdateDetails(item) {
  const data = await operation('core-update', { key: item.id });
  const picture = el('div'), images = data.images.length ? data.images : item.image ? [item.image] : [];
  let at = 0;
  const paint = () => picture.replaceChildren(image(images[at], 'detail-art'));
  paint();
  modal(data.name, picture, images.length > 1 ? button('Next artwork', () => { at = (at + 1) % images.length; paint(); }) : null, el('pre', {}, detailText(data.details)), data.target ? button('Launch', () => launch(data.target), true) : notice(data.installed ? 'This support core is installed. Launch its games from Library.' : 'This core is not installed. Install it with Downloader or Update All.'));
  if (images.length > 1 && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
    const timer = setInterval(() => { if (!$('editor').open || !picture.isConnected) { clearInterval(timer); return; } at = (at + 1) % images.length; paint(); }, 1000);
  }
}
function randomMenu(scope) {
  return button('Random', () => {
    const pick = async favorites_only => {
      closeModal();
      const result = await operation('random', { ...scope, favorites_only });
      if (result.items?.length !== 1) throw new Error('Random selection returned no game.');
      await gameDetails(result.items[0]);
    };
    modal('Random', button('Random Game', () => pick(false)), button('Random Favourite', () => pick(true)));
  });
}
async function coreChoices(system, game = null) {
  const data = await command('core-choices', { system, game }); state.revision = data.revision;
  const family = select([['', game ? 'Follow system' : 'Automatic'], ...data.families.filter(x => x.available).map(x => [x.id, x.label])], data.family || '');
  const version = select([['', 'Automatic'], ...data.versions.map(x => [x.id, x.label])], data.version || '');
  await formModal('Core choice', [field('Compatible core', family), game ? notice('Core version follows this game’s system.') : field('Core version', version)], async () => {
    await edit('core-select', { system, game, family: family.value || null, version: game ? null : version.value || null }); closeModal();
  });
}
async function addFavorite(item) {
  const data = await command('favorite-folders'), folder = select(data.items.map(x => [x.id, x.name])), name = input('');
  await formModal('Add favourite', [field('Folder', folder), field('Filename', name, 'Leave empty to use the normal favourite filename. Game files are not renamed.')], async () => {
    await command('favorite-add', { game: item.id, folder: folder.value || null, name: name.value || null }); closeModal(); await renderRoute();
  }, 'Add favourite');
}
async function favoriteFolders(data = null) {
  data ||= await command('favorite-folders');
  modal('Favourites folders', el('div', { class: 'rows' }, data.items.map(folder => el('div', { class: 'row' }, el('span', { class: 'grow' }, folder.name), folder.id ? el('div', { class: 'actions' }, button('Rename', () => promptName('Rename favourites folder', folder.name, async name => { await favoriteFolders(await command('favorite-folder-rename', { folder: folder.id, name })); })), button('Delete empty folder', async () => { await favoriteFolders(await command('favorite-folder-delete', { folder: folder.id })); })) : null))), button('New folder', () => promptName('New favourites folder', '', async name => { await favoriteFolders(await command('favorite-folder-create', { name })); })));
}
async function folderOptions(target) {
  const data = await command('folder-options', { target }); state.revision = data.revision;
  const rotation = select([['', 'Follow global'], ['off', 'Off'], ['clockwise', '90° Clockwise'], ['counter-clockwise', '90° Counterclockwise']], data.preferences.screen_rotation);
  const folders = select([['', 'Follow global'], ['false', 'Folders first'], ['true', 'Folders last']], data.preferences.folders_last == null ? '' : String(data.preferences.folders_last));
  const names = select([['', 'Follow global'], ['full', 'Full'], ['remove-parentheses', 'Remove parentheses'], ['remove-brackets', 'Remove brackets'], ['remove-parentheses-and-brackets', 'Remove parentheses and brackets'], ['keep-region', 'Keep region'], ['keep-disc-index', 'Keep disc index'], ['keep-region-and-disc-index', 'Keep region and disc index']], data.preferences.game_name_display);
  await formModal('Folder options', [field('Screen rotation', rotation), field('Folder position', folders), field('Game names', names)], async () => {
    await edit('folder-options-save', { target, preferences: { screen_rotation: rotation.value || null, folders_last: folders.value === '' ? null : folders.value === 'true', game_name_display: names.value || null } }); closeModal(); await renderRoute();
  });
}
async function chooseView(target = null) {
  const data = await command('views', { target }); state.revision = data.revision;
  const choices = data.choices.map(key => [key, key.startsWith('custom:') ? data.items[key.slice(7)]?.name || key : key]);
  if (target !== null) choices.unshift(['', 'Follow global']);
  const view = select(choices, target !== null ? data.current || '' : data.global || 'details');
  await formModal('Change MiSTer layout', [field('Layout', view)], async () => { await edit('view-assign', { target, view: view.value }); closeModal(); });
}
async function layoutsPage(epoch) {
  const data = await command('views'); if (epoch !== state.epoch) return; state.revision = data.revision;
  header('Layouts Designer', button('New layout', () => layoutEditor(null, null)), button('Global layout', () => chooseView()));
  const rows = Object.entries(data.items).map(([id, definition]) => el('div', { class: 'row' }, el('div', { class: 'grow' }, el('h3', {}, definition.name), el('small', {}, definition.template)), el('div', { class: 'actions' }, button('Edit', () => layoutEditor(id, definition)), button('Use globally', async () => { await edit('view-assign', { target: null, view: 'custom:' + id }); await renderRoute(); }), button('Delete', async () => { await edit('view-delete', { id }); await renderRoute(); }))));
  $('page').append(notice('Custom layouts use the same panels and proportions on MiSTer. Assign a layout from any system or folder’s actions.'), rows.length ? el('div', { class: 'rows' }, rows) : empty('Create a layout using two columns, stacked panels, or a split column.'));
}
async function layoutEditor(id, source) {
  const definition = structuredClone(source || { name: '', template: 'columns', panels: ['list', 'artwork'], columns: 50, rows: 50 });
  const name = input(definition.name), template = select([['columns', 'Two columns'], ['split-right', 'Left + split right'], ['split-left', 'Split left + right'], ['stacked', 'Two stacked panels']], definition.template);
  const columns = input(definition.columns, 'range'), rows = input(definition.rows, 'range');
  [columns, rows].forEach(control => { control.min = '20'; control.max = '80'; });
  const panelChoices = [['list', 'Browser list'], ['artwork', 'Artwork'], ['short-information', 'Short information'], ['full-information', 'Full information']];
  const selectors = el('div', { class: 'field-grid' }), preview = el('div', { class: 'layout-preview', 'aria-label': 'Layout proportions preview' });
  const paint = () => {
    definition.columns = Number(columns.value); definition.rows = Number(rows.value);
    const x = definition.columns, y = definition.rows, gap = .6;
    const rects = definition.template === 'columns' ? [[0, 0, x, 100], [x, 0, 100 - x, 100]] : definition.template === 'stacked' ? [[0, 0, 100, y], [0, y, 100, 100 - y]] : definition.template === 'split-right' ? [[0, 0, x, 100], [x, 0, 100 - x, y], [x, y, 100 - x, 100 - y]] : [[0, 0, x, y], [0, y, x, 100 - y], [x, 0, 100 - x, 100]];
    preview.replaceChildren();
    rects.forEach(([left, top, width, height], i) => {
      const panel = el('div', { class: 'preview-panel' });
      Object.assign(panel.style, { left: left + '%', top: top + '%', width: (width - gap) + '%', height: (height - gap) + '%' });
      const kind = definition.panels[i];
      const entries=state.previewEntries||[],selected=entries[0];
      if (kind === 'artwork') panel.append(selected?.image?image(selected.image):el('p',{},'Artwork panel'));
      else if (kind === 'list') panel.append(...(entries.length?entries.map((entry,index)=>el('p',{class:index===0?'selected':''},entry.name)):[el('p',{},'Browse a library to preview its entries here.')]));
      else panel.append(el('p', {}, selected?.name||(kind === 'short-information' ? 'Game information' : 'Full game information')), selected?el('small',{},kind==='full-information'?detailText(selected.details):[selected.system_name,selected.genre].filter(Boolean).join(' · ')):null);
      preview.append(panel);
    });
    for (const [axis, control, enabled] of [['vertical', columns, definition.template !== 'stacked'], ['horizontal', rows, definition.template !== 'columns']]) {
      if (!enabled) continue;
      const handle = el('div', { class: 'preview-divider ' + axis, role: 'separator', tabindex: '0', 'aria-label': axis === 'vertical' ? 'Column divider' : 'Row divider', 'aria-valuemin': '20', 'aria-valuemax': '80', 'aria-valuenow': control.value });
      Object.assign(handle.style, axis === 'vertical' ? { left: 'calc(' + control.value + '% - 4px)', top: 0, height: '100%' } : { top: 'calc(' + control.value + '% - 4px)', left: 0, width: '100%' });
      handle.onpointerdown = event => {
        event.preventDefault(); handle.setPointerCapture(event.pointerId);
        const move = event => { const bounds = preview.getBoundingClientRect(); const ratio = axis === 'vertical' ? (event.clientX - bounds.left) / bounds.width : (event.clientY - bounds.top) / bounds.height; control.value = String(Math.round(Math.max(20, Math.min(80, ratio * 100)))); updatePositions(); };
        const done = () => { handle.removeEventListener('pointermove', move); handle.removeEventListener('pointerup', done); updatePositions(); };
        handle.addEventListener('pointermove', move); handle.addEventListener('pointerup', done, { once: true }); handle.addEventListener('pointercancel', done, { once: true });
      };
      handle.onkeydown = event => { if (['ArrowLeft', 'ArrowUp', 'ArrowRight', 'ArrowDown'].includes(event.key)) { event.preventDefault(); control.value = String(Number(control.value) + (['ArrowLeft', 'ArrowUp'].includes(event.key) ? -1 : 1)); updatePositions(); } };
      preview.append(handle);
    }
  };
  const updatePositions = () => {
    // Keep the captured pointer node alive while the browser is dragging.
    definition.columns = Number(columns.value); definition.rows = Number(rows.value);
    const x = definition.columns, y = definition.rows, gap = .6;
    const rects = definition.template === 'columns' ? [[0, 0, x, 100], [x, 0, 100 - x, 100]] : definition.template === 'stacked' ? [[0, 0, 100, y], [0, y, 100, 100 - y]] : definition.template === 'split-right' ? [[0, 0, x, 100], [x, 0, 100 - x, y], [x, y, 100 - x, 100 - y]] : [[0, 0, x, y], [0, y, x, 100 - y], [x, 0, 100 - x, 100]];
    preview.querySelectorAll('.preview-panel').forEach((panel, i) => { const [left, top, width, height] = rects[i]; Object.assign(panel.style, { left: left + '%', top: top + '%', width: (width - gap) + '%', height: (height - gap) + '%' }); });
    const active = preview.querySelector('.preview-divider.vertical'); if (active) { active.style.left = 'calc(' + columns.value + '% - 4px)'; active.setAttribute('aria-valuenow', columns.value); }
    const horizontal = preview.querySelector('.preview-divider.horizontal'); if (horizontal) { horizontal.style.top = 'calc(' + rows.value + '% - 4px)'; horizontal.setAttribute('aria-valuenow', rows.value); }
  };
  const panelForm = () => {
    selectors.replaceChildren();
    definition.panels.forEach((content, at) => {
      const control = select(panelChoices, content);
      control.onchange = () => {
        const previous = definition.panels[at];
        if (control.value === 'list') { const old = definition.panels.indexOf('list'); if (old >= 0) definition.panels[old] = previous; }
        else if (previous === 'list') definition.panels[(at + 1) % definition.panels.length] = 'list';
        definition.panels[at] = control.value; panelForm(); paint();
      };
      selectors.append(field('Panel ' + (at + 1), control));
    });
  };
  template.onchange = () => {
    definition.template = template.value; const count = template.value.startsWith('split') ? 3 : 2;
    definition.panels = count === 3 ? ['list', 'artwork', 'short-information'] : ['list', 'artwork']; panelForm(); paint();
  };
  columns.oninput = updatePositions; rows.oninput = updatePositions; panelForm(); paint();
  await formModal(id ? 'Edit layout' : 'New layout', [field('Name', name), field('Template', template), selectors, preview, el('div', { class: 'field-grid' }, field('Column split', columns), field('Row split', rows))], async () => {
    definition.name = name.value; definition.columns = Number(columns.value); definition.rows = Number(rows.value);
    await edit('view-save', { id, definition }); closeModal(); await renderRoute();
  });
}
async function themesPage(epoch) {
  const data = await command('themes'); if (epoch !== state.epoch) return; state.revision = data.revision;
  header('Themes', button('New theme', () => themeEditor(data, null)), button('Configured palette', async () => { await edit('theme-select', { name: null }); applyState(await command('state')); await renderRoute(); }));
  $('page').append(el('div', { class: 'rows' }, data.items.map(theme => el('div', { class: 'row' }, el('div', { class: 'grow' }, theme.name, theme.name === data.current ? el('small', {}, 'Selected') : null), el('div', { class: 'actions' }, button('Use theme', async () => { await edit('theme-select', { name: theme.name }); applyState(await command('state')); await renderRoute(); }), button(theme.editable ? 'Edit' : 'Copy', () => themeEditor(data, theme)), theme.editable ? button('Delete', async () => { await edit('theme-delete', { name: theme.name }); applyState(await command('state')); await renderRoute(); }) : null)))));
}
async function themeEditor(data, original) {
  const values = {};
  for (const line of (original?.text || '').split('\n')) { const match = line.match(/^([a-z_]+)\s*=\s*(.*)$/); if (match) { try { values[match[1]] = JSON.parse(match[2]); } catch { throw new Error('Theme editor received a value it cannot represent: ' + match[1]); } } }
  const name = input(original?.editable ? original.name : ''), fields = el('div', { class: 'color-fields' }), colors = {};
  for (const key of ['background', 'panel', 'surface', 'bar', 'text', 'text_dim', 'accent', 'accent_text', 'state', 'favorite', 'logo']) {
    const fallback = getComputedStyle(document.documentElement).getPropertyValue('--' + key.replaceAll('_', '-')).trim();
    const control = input(values[key] || (/^#[0-9a-f]{6}$/i.test(fallback) ? fallback : '#ffffff'), 'color'); colors[key] = control; fields.append(field(key.replaceAll('_', ' '), control));
  }
  const font = select([['', 'Follow global'], ['smooth', 'Smooth'], ['smooth 2', 'Smooth 2'], ['pixel', 'Pixel'], ['pixel 2', 'Pixel 2']], values.font || '');
  const size = select(['default', 'smaller', 'small', 'large', 'larger'], values.text_size || 'default'), logo = input(values.logo_opacity ?? 0, 'range'); logo.min = 0; logo.max = 100;
  const preview = el('div', { class: 'theme-preview', 'aria-label': 'Theme preview' });
  const wordmark = el('div', { class: 'theme-wordmark' }, el('img', { src: '/logo.png', alt: 'Degauss', width: 176, height: 58 }));
  const tint = el('span', { 'aria-hidden': 'true' }); wordmark.append(tint);
  const entries = state.previewEntries || [], selected = entries[0];
  const list = el('div', { class: 'theme-preview-list' }, entries.length ? entries.map((entry, index) => el('p', { class: index === 0 ? 'selected' : '' }, entry.name)) : el('p', {}, 'Browse a library to preview its entries here.'));
  preview.replaceChildren(wordmark, el('div', { class: 'theme-preview-body' }, list, image(selected?.image)), el('div', { class: 'theme-preview-bar' }, 'A Play · B Back · X Actions · Y Menu'));
  const paint = () => {
    for (const [key, control] of Object.entries(colors)) preview.style.setProperty('--' + key.replaceAll('_', '-'), control.value);
    preview.style.fontFamily = ({ smooth: 'Degauss', 'smooth 2': 'DegaussHeading', pixel: 'DegaussPixel', 'pixel 2': 'DegaussPixel2' })[font.value] || getComputedStyle(document.body).fontFamily;
    preview.style.fontSize = ({ smaller: '14px', small: '15px', default: '16px', large: '18px', larger: '20px' })[size.value];
    tint.style.opacity = Number(logo.value) / 100;
  };
  for (const control of [...Object.values(colors), font, size, logo]) control.addEventListener('input', paint);
  paint();
  await formModal(original?.editable ? 'Edit theme' : 'New theme', [field('Name', name), preview, fields, field('Text', font), field('Text size', size), field('Logo tint amount', logo)], async () => {
    const result = ['created_by_editor = true'];
    if (font.value) result.push('font = ' + JSON.stringify(font.value));
    result.push('text_size = ' + JSON.stringify(size.value));
    for (const [key, control] of Object.entries(colors)) result.push(key + ' = ' + JSON.stringify(control.value));
    result.push('logo_opacity = ' + logo.value);
    await edit('theme-save', { name: name.value, text: result.join('\n') + '\n', replace: Boolean(original?.editable && name.value === original.name) }); closeModal(); await renderRoute(); applyState(await command('state'));
  });
}
async function chooseImage(target) {
  const data = await command('images'); state.revision = data.revision;
  modal('Choose image', uploadControl(target), button('Use default image', async () => { await edit('image-choose', { target, image: null }); closeModal(); await renderRoute(); }), renderTiles(data.items, async item => { await edit('image-choose', { target, image: item.id }); closeModal(); await renderRoute(); }));
}

function uploadControl(target) {
  const file = input('', 'file'); file.accept = 'image/png,image/jpeg';
  const send = button('Upload image', async () => {
    const selected = file.files[0];
    if (!selected) throw new Error('Choose a PNG or JPEG image first.');
    if (selected.size > 16 * 1024 * 1024) throw new Error('Artwork must be no larger than 16 MiB.');
    const revision = $('editor').open ? $('editor').dataset.revision : state.revision;
    let response;
    try { response = await fetch('/api/v1/upload/' + encodeURIComponent(target), { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': selected.type || 'application/octet-stream', 'X-Degauss-CSRF': state.csrf, 'X-Degauss-Revision': revision }, body: selected, signal: AbortSignal.timeout(30000) }); }
    catch { throw new Error('Artwork upload was not acknowledged. Check its current state before trying again.'); }
    const result = await response.json();
    if (!response.ok) throw new Error(result.error?.message || 'Artwork upload failed.');
    closeModal(); if (result.job) await waitJob(result.job); await renderRoute();
  });
  return el('div', { class: 'stack' }, field('Upload artwork', file, 'PNG or JPEG, up to 16 MiB and 4096 pixels per side. Game files are never uploaded.'), send);
}
async function gameData(system) {
  const data = await command('game-data', { system }); state.revision = data.revision;
  if (!data.supports_pack) { modal('Game data source', notice('This system uses Gamelist metadata and artwork. No Artwork Pack definition is available for this system.')); return; }
  const source = select([['automatic', 'Automatic'], ['gamelist', 'Gamelist'], ...(data.supports_pack ? [['artwork-pack', 'Artwork Pack']] : [])], data.source);
  const kind = select([['', 'Follow global pack type'], ['box-art', 'Box Art'], ['screenshot', 'Screenshot'], ['title-screen', 'Title Screen']], data.inherit_type ? '' : data.picture_type);
  let selected = null; const selectedLabel = el('p', { class: 'muted' }, data.location || 'No explicit pack location selected'), chooser = el('div');
  const list = async directory => {
    const locations = await command('game-data-locations', { system, picture_type: kind.value || data.picture_type, directory });
    chooser.replaceChildren(button('Storage roots', () => list(null)), el('div', { class: 'rows' }, locations.items.map(item => el('div', { class: 'row' }, el('span', { class: 'grow' }, item.name, el('small', {}, item.location)), button('Open', () => list(item.id)), item.selectable ? button('Use', () => { selected = item.id; selectedLabel.textContent = item.location; chooser.replaceChildren(); }) : null))));
  };
  const pick = button('Choose pack location', () => list(null));
  const refreshControls = () => { kind.disabled = source.value !== 'artwork-pack'; pick.disabled = source.value !== 'artwork-pack'; };
  source.onchange = refreshControls; kind.onchange = () => { selected = null; selectedLabel.textContent = 'Choose a location for the selected pack type.'; chooser.replaceChildren(); }; refreshControls();
  await formModal('Game data source', [notice('Currently using ' + data.effective + '. This choice controls both metadata and artwork.'), data.error ? notice(data.error) : null, field('Source', source), field('Artwork Pack type', kind), selectedLabel, pick, chooser], async () => {
    const response = await edit('game-data-save', { system, source: source.value, location: selected, picture_type: kind.value || null }); closeModal();
    if (response.job) await waitJob(response.job); await renderRoute();
  });
}
async function artworkActions(item) {
  const source = await command('game-data', { system: item.system });
  if (source.effective === 'artwork-pack') return packMatcher(item);
  const term = input(item.name), results = el('div');
  const search = async () => {
    results.replaceChildren(notice('Searching the selected scraper…'));
    const data = await operation('scraper-search', { game: item.id, term: term.value });
    results.replaceChildren(data.failed_searches ? notice(data.failed_searches + ' searches failed.') : document.createDocumentFragment(), renderTiles(data.items, async match => {
      const preview = match.image_available ? await operation('scraper-preview', { match_id: match.id }) : null;
      const canvas = el('canvas', { class: 'inline-image' });
      if (preview) {
        canvas.width = preview.width; canvas.height = preview.height; const ctx = canvas.getContext('2d'), pixels = ctx.createImageData(preview.width, preview.height);
        for (let i = 0, j = 0; i < preview.rgb.length; i += 3, j += 4) { pixels.data[j] = preview.rgb[i]; pixels.data[j + 1] = preview.rgb[i + 1]; pixels.data[j + 2] = preview.rgb[i + 2]; pixels.data[j + 3] = 255; } ctx.putImageData(pixels, 0, 0);
      }
      modal(match.name, preview ? el('span', { class: 'web-image' }, canvas) : notice('No image in this match.'), el('pre', {}, detailText(match.metadata)), button('Save this match', async () => { closeModal(); await operation('scrape', { target: item.id, match_id: match.id }); await renderRoute(); }), button('Back to results', () => artworkActions(item)));
    }, null, true));
  };
  modal('Artwork for ' + item.name, field('Search title', term), el('div', { class: 'actions' }, button('Search', search, true), button('Scrape this game', () => scrapeConfirm(item.id, item.name)), button('Scraper settings', scraperSettings)), uploadControl(item.id), results);
}
async function packMatcher(item) {
  const term = input('', 'search'), results = el('div');
  const search = async () => {
    const data = await operation('pack-matches', { game: item.id, search: term.value }); state.revision = data.revision;
    results.replaceChildren(renderTiles(data.items, match => modal(match.name, image(match.image, 'detail-art'), el('pre', {}, detailText(match.details)), button('Use artwork', async () => { const response = await edit('pack-match-save', { game: item.id, choice: match.id }); closeModal(); if (response.job) await waitJob(response.job); await renderRoute(); })), null, true));
  };
  modal('Match Artwork Pack', field('Find installed artwork', term), el('div', { class: 'actions' }, button('Search', search), button('Reset manual match', async () => { const response = await edit('pack-match-save', { game: item.id, choice: null }); closeModal(); if (response.job) await waitJob(response.job); await renderRoute(); })), results);
}
async function scrapeConfirm(target, name) {
  modal('Scrape ' + name, notice('Uses your selected scraper and image/metadata policies. A manually selected single game replaces a found image even with Missing Only. Artwork Pack sources are not switched.'), el('div', { class: 'actions' }, button('Start scraping', async () => { closeModal(); const result = await operation('scrape', { target, match_id: null }); modal('Scrape results', el('pre', {}, detailText(result))); }, true), button('Scraper settings', scraperSettings)));
}
async function scraperSettings() {
  const data = await command('scraper-settings'); state.revision = data.revision; const values = data.values, controls = {};
  const options = { source: [['screen_scraper', 'ScreenScraper'], ['libretro', 'Libretro']], libretro_artwork: [['screenshot', 'Screenshots'], ['box_art', 'Box Art'], ['title_screen', 'Title Screens']], image_policy: [['off', 'Off'], ['missing_only', 'Missing only'], ['replace_existing', 'Replace existing']], metadata_policy: [['off', 'Off'], ['fill_missing', 'Fill missing'], ['replace_existing', 'Replace existing']] };
  const fields = [];
  for (const [key, value] of Object.entries(values)) {
    if (value && typeof value === 'object') continue;
    const control = options[key] ? select(options[key], value) : input(value ?? '', typeof value === 'number' ? 'number' : 'text'); controls[key] = control;
    fields.push(field(key.replaceAll('_', ' '), control));
  }
  const password = input('', 'password'), replacePassword = input('', 'checkbox'), consent = input('', 'checkbox');
  const mappings = el('details', {}, el('summary', {}, 'Per-system ScreenScraper preferences')), mappingControls = new Map();
  for (const system of state.systems) {
    const platform = input(values.system_ids?.[system.system] ?? '', 'number'); platform.min = 1;
    const media = input(values.system_media_types?.[system.system] || '');
    mappings.append(el('div', { class: 'field-grid' }, field(system.name + ': platform ID', platform, 'Empty uses the built-in mapping.'), field('Media type', media, 'Empty uses the global media type.')));
    mappingControls.set(system.system, { platform, media });
  }
  fields.push(mappings);
  fields.push(field('ScreenScraper password', password, data.password_set ? 'A password is saved. Leave unchanged unless replacing it.' : 'No password saved.'), el('label', { class: 'check' }, replacePassword, 'Replace saved password'), el('label', { class: 'check' }, consent, 'Allow ScreenScraper credentials to be stored as plain text on the card'));
  await formModal('Scraper settings', fields, async () => {
    const edited = {};
    for (const [key, control] of Object.entries(controls)) edited[key] = typeof values[key] === 'number' ? Number(control.value) : (['region', 'language'].includes(key) ? control.value || null : control.value);
    edited.system_ids = { ...values.system_ids }; edited.system_media_types = { ...values.system_media_types };
    for (const [system, controls] of mappingControls) {
      if (controls.platform.value) edited.system_ids[system] = Number(controls.platform.value); else delete edited.system_ids[system];
      if (controls.media.value) edited.system_media_types[system] = controls.media.value; else delete edited.system_media_types[system];
    }
    await edit('scraper-settings-save', { values: edited, password: replacePassword.checked ? password.value : null, accept_plaintext_storage: consent.checked }); closeModal();
  });
}
async function settingsPage(epoch) {
  const data = await command('options'); if (epoch !== state.epoch) return; state.revision = data.revision;
  header('Settings', state.locked ? button('Unlock kiosk', unlock) : null);
  for (const page of data.pages) {
    const section = el('details', { open: page.key === 'appearance', 'data-key': page.key }, el('summary', {}, page.name)), rows = el('div');
    for (const option of page.items) {
      const actions = el('div', { class: 'actions' });
      if (option.adjustable && option.id !== 'WebMode') {
        for (const [label, delta] of [['−', -1], ['+', 1]]) {
          const change = button(label, async () => { const response = await edit('option-adjust', { option: option.id, delta }); if (response.job) { const result = await waitJob(response.job); if (Object.keys(result.source_errors || {}).length) throw new Error(Object.values(result.source_errors).join('\n')); } applyState(await command('state')); await renderRoute(); }); change.setAttribute('aria-label', option.name + (delta < 0 ? ': previous' : ': next')); actions.append(change);
        }
      } else {
        const run = optionEditor(option.id);
        if (run) actions.append(button('Open', run));
        else if (option.id !== 'Spacer') throw new Error('No browser editor is registered for ' + option.name + '.');
      }
      rows.append(el('div', { class: 'option' }, el('div', { class: 'grow' }, option.name, el('small', {}, option.help)), el('span', { class: 'value' }, option.value), actions));
    }
    section.append(rows); $('page').append(section);
  }
  $('page').append(el('div', { class: 'actions' }, button('Scraper settings', scraperSettings), button('Saved controllers', controllersEditor), button('Rebuild library', async () => { await operation('rebuild', { system: null }); await renderRoute(); }), button('Reset view selections', async () => { await edit('reset', { kind: 'view-selections' }); await renderRoute(); }), button('Unhide entries', async () => { await edit('reset', { kind: 'hidden' }); await renderRoute(); })));
}
function optionEditor(id) {
  return ({ CustomViews: () => go('layouts'), Theme: () => go('themes'), Controllers: controllersEditor, ScraperSettings: scraperSettings, RebuildCache: () => operation('rebuild', { system: null }), ScrapeAll: () => scrapeConfirm(null, 'all systems'), ResetCustomViews: async () => { await edit('reset', { kind: 'view-selections' }); await renderRoute(); }, ResetHidden: async () => { await edit('reset', { kind: 'hidden' }); await renderRoute(); }, SwitchIni: iniProfiles, Advanced: () => document.querySelector('details[data-key=advanced]')?.setAttribute('open', ''), WebMode: webSettings, WebPort: webSettings, WebPassword: webSettings, WebAddress: webSettings, WebCertificate: webSettings })[id];
}

async function iniProfiles() {
  const data = await operation('ini-profiles');
  const slot = select(data.profiles.map(profile => [profile.slot, profile.name]), data.current);
  await formModal('MiSTer INI profile', [field('Profile', slot), notice('Switching reloads MiSTer’s current core or frontend. The selected video settings may leave a display without a signal.')], async () => {
    await command('ini-switch', { slot: Number(slot.value) }); closeModal(); state.waiting = true; offline();
  }, 'Switch profile');
}
async function controllersEditor() {
  const data = await command('controllers'); state.revision = data.revision;
  const entries = Object.entries(data.profiles);
  const rows = entries.map(([id, profile]) => el('div', { class: 'row' }, el('div', { class: 'grow' }, id, el('small', {}, profile.custom ? 'Degauss mapping' : 'MiSTer mapping')), button('Edit', () => {
    const custom = input('', 'checkbox'); custom.checked = profile.custom;
    const bindingChoices = Object.entries(profile.bindings), fields = [el('label', { class: 'check' }, custom, 'Use independent Degauss mapping')], controls = {};
    bindingChoices.forEach(([role, bindings]) => { const dropdown = select(bindingChoices.map(([key, bindings]) => [key, bindings.map(b => b.label).join(' + ') || 'Unassigned']), role); controls[role] = dropdown; fields.push(field(role, dropdown)); });
    return formModal('Saved controller', fields, async () => {
      const bindings = {}; for (const [role, dropdown] of Object.entries(controls)) bindings[role] = profile.bindings[dropdown.value];
      const response = await edit('controller-save', { id, profile: { custom: custom.checked, bindings } }); closeModal(); if (response.job) await waitJob(response.job);
    });
  })));
  modal('Controller preferences', notice(data.capture), rows.length ? el('div', { class: 'rows' }, rows) : empty('No saved controllers. Capture a controller’s buttons on MiSTer first.'));
}
async function unlock() {
  const pin = input('', 'password'); pin.inputMode = 'numeric';
  await formModal('Unlock kiosk mode', [field('Kiosk PIN', pin)], async () => { await command('unlock', { pin: pin.value }); closeModal(); applyState(await command('state')); await renderRoute(); }, 'Unlock');
}
async function webSettings() {
  const data = await command('web-settings');
  const mode = select([['off', 'Off'], ['https', 'HTTPS'], ['http', 'HTTP (unencrypted)']], data.config.mode), port = input(data.config.port, 'number'), password = input('', 'password');
  port.min = 1024; port.max = 65535; password.autocomplete = 'new-password';
  await formModal('Web Access', [field('Mode', mode), field('Port', port), field('New password', password, 'Leave empty to keep the current password. Changing it signs out all browsers.'), notice('Changing the mode or port disconnects this page. Reopen the address shown on MiSTer.'), el('pre', {}, data.addresses.join('\n')), data.error ? notice(data.error) : null], async () => {
    const response = await edit('web-settings-save', { config: { mode: mode.value, port: Number(port.value) }, password: password.value || null }); closeModal(); if (response.job) await waitJob(response.job);
    signedOut(); $('login-error').textContent = 'Web Access changed. Open the address shown on MiSTer and sign in again.';
  }, 'Save changes', [button('Renew HTTPS certificate', async () => { await command('web-certificate-renew'); signedOut(); closeModal(); $('login-error').textContent = 'The certificate has changed. Reopen this address and trust the new certificate before signing in.'; })]);
}
function remotePage() {
  header('Remote Control', button('Start Attract Mode', () => command('attract')));
  const pad = el('div', { class: 'remote-pad' });
  const send = async control => { const reply = await command('remote', { button: control }); if (reply.leaving) { state.waiting = true; offline(); } };
  for (const [label, action] of [['', null], ['↑', 'up'], ['', null], ['←', 'left'], ['Select', 'select'], ['→', 'right'], ['Back', 'back'], ['↓', 'down'], ['Actions', 'actions']]) pad.append(action ? button(label, () => send(action)) : el('span'));
  $('page').append(notice('These controls operate the MiSTer screen. Browsing the other web pages does not move its selection.'), el('p', { id: 'native-state', class: 'status-line', 'aria-live': 'polite' }, [state.native.screen, state.native.system, state.native.selected].filter(Boolean).join(' · ')), pad, button('Menu', () => send('menu')));
}
