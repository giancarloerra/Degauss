import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

// Isolate the shared dropdown constructor. Real browser/device journeys cover
// DOM rendering; this models the native select's value/selection contract.
const source = readFileSync(new URL('../web/app.js', import.meta.url), 'utf8');
const start = source.indexOf('function select(');
const end = source.indexOf('\nfunction showError(', start);
assert.ok(start >= 0 && end > start);
const context = vm.createContext({
  el(tag, attrs, label) {
    if (tag === 'option') return { value: String(attrs.value), label };
    assert.equal(tag, 'select');
    return {
      options: [], selectedIndex: -1,
      append(option) {
        this.options.push(option);
        if (this.selectedIndex < 0) this.selectedIndex = 0;
      },
      get value() { return this.options[this.selectedIndex]?.value ?? ''; },
      set value(value) { this.selectedIndex = this.options.findIndex(option => option.value === String(value)); },
    };
  },
});
vm.runInContext(source.slice(start, end), context);

test('Core Updates initially shows all cores, sorted by latest build', () => {
  assert.equal(context.select([['all', 'All cores'], ['changed', "What's New"]]).value, 'all');
  assert.equal(context.select([['latest', 'Latest build'], ['name', 'Name']]).value, 'latest');
});
test('Explicit inherited, null and numeric selections retain their meaning', () => {
  const choices = [['', 'Follow global'], ['list', 'List']];
  assert.equal(context.select(choices, '').selectedIndex, 0);
  assert.equal(context.select(choices, null).selectedIndex, 0);
  assert.equal(context.select(choices, 'list').value, 'list');
  assert.equal(context.select([[0, 'Main'], [1, 'Alternate']], 1).value, '1');
});
test('Missing explicit selection is not silently replaced', () => {
  assert.equal(context.select([['list', 'List']], 'deleted').selectedIndex, -1);
});

function section(from, to) {
  const start = source.indexOf(from), end = source.indexOf(to, start);
  assert.ok(start >= 0 && end > start);
  return source.slice(start, end);
}

test('A missing selected theme is visibly reported on the Themes page', async () => {
  const displayed = [];
  const context = vm.createContext({
    state: { epoch: 1 },
    command: async () => ({ revision: 'theme-revision', items: [], notices: ['Theme Amber did not load; using standard.'] }),
    header() {}, button() {}, el: () => ({}),
    notice: message => { displayed.push(message); return {}; },
    $: () => ({ append() {} }),
  });
  vm.runInContext(section('async function themesPage(', '\nasync function themeEditor('), context);
  await context.themesPage(1);
  assert.deepEqual(displayed, ['Theme Amber did not load; using standard.']);
});

test('An expired artwork-upload session closes its editor and returns to sign-in without retrying', async () => {
  for (const status of [401, 409]) {
    const elements = new Map([['editor', { open: true, dataset: { revision: 'revision' } }], ['login', { hidden: true }], ['shell', { hidden: false }], ['transport', {}], ['login-error', {}]]);
    let upload, requests = 0, closes = 0;
    const context = vm.createContext({
      $: id => elements.get(id), state: { csrf: 'session' }, location: { protocol: 'https:' }, AbortSignal,
      closeModal: () => { closes++; elements.get('editor').open = false; },
      input: () => ({ files: [{ size: 10, type: 'image/png' }] }),
      button: (label, run) => { upload = run; return {}; }, el: () => ({}), field: () => ({}),
      fetch: async () => { requests++; return { ok: false, status, json: async () => ({ error: { code: status === 401 ? 'auth' : 'conflict', message: 'Request refused' } }) }; },
      waitJob: () => assert.fail('Failed upload must not poll a job'), renderRoute: () => assert.fail('Failed upload must not repaint a success'),
    });
    vm.runInContext(section('function signedOut(', '\nfunction applyState('), context);
    vm.runInContext(section('function uploadControl(', '\nasync function gameData('), context);
    context.uploadControl('artwork-target');
    if (status === 401) {
      await upload();
      assert.equal(closes, 1); assert.equal(elements.get('login').hidden, false);
      assert.equal(elements.get('login-error').textContent, 'Request refused'); assert.equal(context.state.csrf, '');
    } else {
      await assert.rejects(upload(), error => error.code === 'conflict');
      assert.equal(closes, 0); assert.equal(elements.get('editor').open, true);
    }
    assert.equal(requests, 1);
  }
});

test('Random menu preserves folder or collection filters, distinguishes favourites, and opens details without launching', async () => {
  for (const scope of [{ folder: 'folder-id', query: { title: '1942', fields: { Genre: { Known: { key: 'shooter', label: 'Shooter' } } } } }, { query: { system: 'NES' } }]) {
    const calls = [], picks = [];
    let actions;
    const context = vm.createContext({
      button: (label, run) => ({ label, run }),
      modal: (title, ...buttons) => { assert.equal(title, 'Random'); actions = buttons; },
      closeModal() {},
      operation: async (action, value) => { calls.push({ action, value: JSON.parse(JSON.stringify(value)) }); return { items: [{ id: 'picked-game' }] }; },
      gameDetails: async item => picks.push(item.id),
    });
    vm.runInContext(section('function randomMenu(', '\nasync function coreChoices('), context);
    const menu = context.randomMenu(scope);
    assert.equal(menu.label, 'Random'); menu.run();
    assert.deepEqual(actions.map(action => action.label), ['Random Game', 'Random Favourite']);
    await actions[0].run(); await actions[1].run();
    assert.deepEqual(calls, [false, true].map(favorites_only => ({ action: 'random', value: { ...scope, favorites_only } })));
    assert.deepEqual(picks, ['picked-game', 'picked-game']);
  }
  assert.ok(section('async function browsePage(', '\nfunction filterPanel(').includes('randomMenu({ folder: data.folder'));
  assert.ok(section('async function explorePage(', '\nasync function gameDetails(').includes('randomMenu({ query })'));
});

test('Browser view changes redraw connected pages without changing native settings or fetching lists', () => {
  let paints = 0;
  const context = vm.createContext({ state: { webView: 'grid', browsers: [{ root: { isConnected: true }, paint: () => paints++ }, { root: { isConnected: false }, paint: () => assert.fail('Detached page') }] } });
  vm.runInContext(section('function setWebView(', '\nfunction paginate('), context);
  for (const mode of ['list', 'details', 'gallery', 'carousel', 'grid']) {
    context.setWebView(mode); assert.equal(context.state.webView, mode);
  }
  assert.equal(paints, 5);
  assert.equal(context.state.browsers.length, 1);
  assert.throws(() => context.setWebView('native-custom-layout'), /Unknown browser view/);
  assert.ok(!source.includes('Choose layout'));
  assert.ok(source.includes('Change MiSTer layout'));
});

test('CRT image state toggles only the browser CSS class and never replaces images', () => {
  const toggles = [];
  const context = vm.createContext({
    state: {}, $: () => null,
    document: { documentElement: { classList: { toggle: (...args) => toggles.push(args) } } },
  });
  vm.runInContext(section('function applyState(', '\nasync function connected('), context);
  context.applyState({ crt_images: true });
  context.applyState({ crt_images: false });
  context.applyState({ locked: true });
  assert.deepEqual(toggles, [['crt-images', true], ['crt-images', false]]);
  assert.equal(context.state.locked, true);
  const css = readFileSync(new URL('../web/style.css', import.meta.url), 'utf8');
  assert.ok(css.includes('.crt-images :is(.web-image,.home-link,.theme-wordmark)'));
  assert.ok(!css.includes('.crt-images img'));
});
function navigationContext() {
  const elements = new Map();
  const $ = id => {
    if (!elements.has(id)) elements.set(id, {
      textContent: '', hidden: false, attrs: {}, children: [],
      classList: { remove() {} },
      replaceChildren(...children) { this.children = children; },
      setAttribute(key, value) { this.attrs[key] = value; },
      removeAttribute(key) { delete this.attrs[key]; },
    });
    return elements.get(id);
  };
  const context = vm.createContext({
    $, elements, state: { csrf: 'session', epoch: 0 },
    location: { hash: '#home' }, document: { querySelectorAll: () => [] },
    el: (tag, attrs = {}, ...children) => ({ tag, attrs, children: children.flat(Infinity) }),
  });
  vm.runInContext(section('function routeHref(', '\nfunction detailText('), context);
  vm.runInContext(section('function progressText(', '\nasync function waitJob('), context);
  return context;
}
test('Breadcrumbs link real ancestor routes and identify the current folder', () => {
  const context = navigationContext();
  context.breadcrumbs([{ name: 'Consoles', page: 'library', data: { category: 'Console' } }, { name: 'NES', page: 'browse', data: { system: 'NES' } }]);
  const rows = context.$('breadcrumb').children[0].children;
  assert.equal(rows[0].children[0].attrs.href, '#home');
  assert.equal(rows[1].children[0].attrs.href, '#library/' + encodeURIComponent(JSON.stringify({ category: 'Console' })));
  assert.equal(rows[2].children[0].attrs['aria-current'], 'page');
  assert.equal(rows[2].children[0].children[0], 'NES');
});
test('Single-system Home categories open games, with Favourites controls only in Favourites', async () => {
  const context = navigationContext();
  const systems = [{ system: 'Arcade', category: 'Arcade' }, { system: 'Favorites', category: 'Favorites' }, { system: 'NES', category: 'Console' }, { system: 'SNES', category: 'Console' }];
  context.command = async () => ({ items: systems });
  context.browsePage = route => route.system;
  vm.runInContext(section('async function libraryPage(', '\nasync function browsePage('), context);
  assert.equal(await context.libraryPage({ category: 'Arcade' }, 0), 'Arcade');
  assert.equal(await context.libraryPage({ category: 'Favorites' }, 0), 'Favorites');
  assert.equal(context.categorySystems(systems, 'Console').length, 2);
  assert.equal(context.isFavorites(systems[0]), false);
  assert.equal(context.isFavorites(systems[1]), true);
  assert.equal(context.isFavorites({ category: 'Favourites' }), true);
});
test('Library progress keeps an explicit Reading label, including bare source names', () => {
  const context = navigationContext();
  assert.equal(context.progressText('NES · 420 games', 'Reading'), 'Reading: NES · 420 games');
  assert.equal(context.progressText('', 'Reading'), 'Reading…');
  assert.equal(context.progressText('Saving artwork'), 'Working: Saving artwork');
  assert.equal(context.progressText('Reading library', 'Reading'), 'Reading library');
});
test('Long-running progress stays visible inside an editor and returns to the page on close', () => {
  const context = navigationContext();
  vm.runInContext(section('function placeJobs(', '\nasync function formModal('), context);
  const row = {};
  context.state.jobs = new Map([[1, row]]);
  let destination;
  context.$('editor-body').prepend = value => { assert.equal(value, row); destination = 'editor'; };
  context.$('jobs').prepend = value => { assert.equal(value, row); destination = 'page'; };
  context.$('editor').open = true; context.placeJobs();
  assert.equal(destination, 'editor');
  context.$('editor').open = false; context.placeJobs();
  assert.equal(destination, 'page');
});
test('Navigation visibly loads before an async page and clears the state on success or failure', async () => {
  const context = navigationContext();
  vm.runInContext(section('async function renderRoute(', "\ndocument.querySelectorAll('.home-link')"), context);
  let finish;
  context.homePage = () => new Promise(resolve => { finish = resolve; });
  context.remotePage = () => {};
  context.showError = error => { context.error = error; };
  const pending = context.renderRoute();
  assert.equal(context.$('page-loading').hidden, false);
  assert.equal(context.$('page-loading').textContent, 'Loading Home…');
  assert.equal(context.$('page').attrs['aria-busy'], 'true');
  finish(); await pending;
  assert.equal(context.$('page-loading').hidden, true);
  assert.equal(context.$('page').attrs['aria-busy'], undefined);
  context.homePage = async () => { throw new Error('Read failed'); };
  await context.renderRoute();
  assert.equal(context.error.message, 'Read failed');
  assert.equal(context.$('page-loading').hidden, true);
});
test('Both logos link Home and Collections describes its Explore action', () => {
  const html = readFileSync(new URL('../web/index.html', import.meta.url), 'utf8');
  assert.equal((html.match(/href="#home" aria-label="Degauss Home"/g) || []).length, 2);
  assert.ok(source.includes("button('Explore games to create or edit'"));
  assert.ok(!section('async function libraryPage(', '\nasync function browsePage(').includes('favoriteFolders'));
});

test('Pagination keeps Previous/count/Next above page and letter jumps, preserving filtered offsets and empty results', () => {
  const context = vm.createContext({
    button: (label, run) => ({ label, run }), field: (label, node) => ({ label, node }),
    select: (choices, value) => ({ choices, value: String(value) }),
    el: (tag, attrs, ...children) => ({ tag, attrs, children }),
  });
  vm.runInContext(section('function paginate(', '\nfunction notices('), context);
  const offsets = [];
  const result = context.paginate({ total: 125, offset: 0, limit: 60, letters: [{ letter: 'Z', offset: 124 }] }, offset => offsets.push(offset));
  const [navigation, jumps] = result.children;
  assert.equal(navigation.attrs.class, 'pagination-navigation');
  assert.equal(jumps.attrs.class, 'pagination-jumps');
  assert.equal(navigation.children[0].label, 'Previous');
  assert.equal(navigation.children[1].children[0], '1–60 / 125');
  assert.equal(navigation.children[2].label, 'Next');
  assert.deepEqual(Array.from(jumps.children, entry => entry.label), ['Page', 'Letter']);
  const page = jumps.children[0].node, letter = jumps.children[1].node;
  assert.deepEqual(Array.from(page.choices, pair => Array.from(pair)), [['', 'Jump to page'], [0, '1'], [60, '2'], [120, '3']]);
  page.value = '120'; page.onchange(); letter.value = '124'; letter.onchange();
  assert.deepEqual(offsets, [120, 124]);
  const empty = context.paginate({ total: 0, offset: 0, limit: 60, letters: [] }, () => assert.fail());
  assert.ok(empty.children[0].children[0].disabled && empty.children[0].children[2].disabled);
  assert.equal(empty.children[0].children[1].children[0], '0 entries');
  assert.ok(empty.children[1].children[0].node.disabled && empty.children[1].children[1].node.disabled);
  const letters = context.letterOffsets([{ name: 'Zoo' }, { name: 'alpha' }, { name: 'Apple' }, { name: '!Other' }, { name: '1941' }]);
  assert.equal(JSON.stringify(letters), JSON.stringify([{ letter: '#', offset: 3 }, { letter: '1', offset: 4 }, { letter: 'A', offset: 1 }, { letter: 'Z', offset: 0 }]));
});

test('Existing completion events wake job reads without waiting for the poll timer', async () => {
  let timer, cleared = false;
  const context = vm.createContext({ state: { jobSignals: new Map() }, setTimeout: fn => { timer = fn; return 1; }, clearTimeout: id => { assert.equal(id, 1); cleared = true; } });
  vm.runInContext(section('function jobChanged(', '\nasync function waitJob('), context);
  const signal = { changed: false, wake: null }; context.state.jobSignals.set(7, signal);
  const pending = context.pauseJob(signal);
  context.jobChanged(7); await pending;
  assert.ok(cleared); assert.equal(signal.wake, null);
  // A completion arriving during the HTTP read must also avoid the timer.
  timer = null; await context.pauseJob(signal); assert.equal(timer, null);
  signal.changed = false; const progress = context.pauseJob(signal); timer(); await progress;
});

test('Game click immediately opens real selected data, and a completed read never reopens a closed dialog', async () => {
  const context = navigationContext(); let complete;
  context.operation = () => new Promise(resolve => { complete = resolve; });
  context.image = path => ({ image: path }); context.detailText = details => details?.desc || '';
  context.button = (label, run) => ({ label, run });
  context.el = (tag, attrs = {}, ...children) => ({ tag, attrs, children, removeAttribute(name) { delete this.attrs[name]; } });
  context.modal = (name, content) => { context.shown = { name, content }; context.$('editor').open = true; };
  context.$('editor-body').contains = node => node === context.shown.content;
  vm.runInContext(section('async function gameDetails(', '\nasync function gameActions('), context);
  const pending = context.gameDetails({ id: 'game', name: 'Actual game', image: '/art', details: { desc: 'Actual excerpt' } });
  assert.equal(context.shown.name, 'Actual game');
  assert.equal(context.shown.content.attrs['aria-busy'], 'true');
  assert.equal(context.shown.content.children[1].children[2].children[0].label, 'Launch');
  context.$('editor').open = false;
  complete({ name: 'Full game', details: { desc: 'Full text' }, revision: 'new' }); await pending;
  assert.equal(context.$('editor').open, false);
  assert.equal(context.shown.content.attrs['aria-busy'], undefined);
});

test('Preview adjustments retain images, and favourite edits use their returned folder list', async () => {
  const themePaint = section('  const paint = () => {\n    for (const [key, control] of Object.entries(colors))', '\n  for (const control of [...Object.values(colors)');
  assert.ok(!themePaint.includes('image(') && !themePaint.includes('replaceChildren'));
  const layout = section('async function layoutEditor(', '\nasync function themesPage(');
  assert.ok(layout.includes('columns.oninput = updatePositions; rows.oninput = updatePositions'));
  const context = navigationContext();
  context.button = (label, run) => ({ label, run }); context.modal = (...args) => { context.dialog = args; };
  context.command = async () => assert.fail('Already refreshed folder list must not be fetched again');
  vm.runInContext(section('async function favoriteFolders(', '\nasync function folderOptions('), context);
  await context.favoriteFolders({ items: [{ id: '', name: 'Root' }] });
  assert.equal(context.dialog[0], 'Favourites folders');
});
