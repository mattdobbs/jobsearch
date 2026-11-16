'use strict';

const COLUMNS = [
  { id: 'new', label: 'New', empty: 'Nothing new right now.' },
  { id: 'evaluating', label: 'Evaluating', empty: 'Drag a card here while you look into it.' },
  { id: 'applied', label: 'Applied to', empty: 'Drag a card here once you have applied.' },
  { id: 'pass', label: 'Pass', empty: 'Jobs you pass on stay here and are never shown as new again.' },
];

// The one-click moves offered on a card, per column.
const QUICK_MOVES = {
  new: [['evaluating', 'Evaluate'], ['pass', 'Pass']],
  evaluating: [['applied', 'Applied'], ['pass', 'Pass']],
  applied: [['pass', 'Pass']],
  pass: [['new', 'Restore']],
};

const WHY_LABELS = {
  program: 'Neurodiversity hiring program',
  index: 'Disability inclusion index',
  board: 'Autism job board',
  signal: 'Says so in the posting',
};

const PAGE_SIZE = 100;

const state = {
  jobs: [],
  filter: '',
  shown: {},
  status: null,
  wasScanning: false,
  openJob: null,
};

const $ = (selector) => document.querySelector(selector);

/** Builds an element. Text is always set as text, never parsed as HTML. */
function h(tag, props, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === 'class') el.className = value;
    else if (key.startsWith('on')) el.addEventListener(key.slice(2), value);
    else if (key in el && key !== 'list' && key !== 'form') el[key] = value;
    else el.setAttribute(key, value === true ? '' : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

async function api(method, path, body, headers) {
  const options = { method, headers: { ...headers } };
  if (body instanceof ArrayBuffer) {
    options.body = body;
  } else if (body !== undefined) {
    options.headers['Content-Type'] = 'application/json';
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  if (!response.ok) {
    let message = `Request failed (${response.status})`;
    try {
      message = (await response.json()).error || message;
    } catch (_) { /* not JSON */ }
    throw new Error(message);
  }
  return response.json();
}

let toastTimer = null;
function toast(message) {
  const el = $('#toast');
  el.textContent = message;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 5000);
}

function fail(error) {
  toast(error.message || String(error));
}

/** Asks before doing something that cannot be undone. Resolves to true or false. */
function confirmAction(message, actionLabel) {
  const dialog = $('#confirm-dialog');
  $('#confirm-message').textContent = message;
  $('#confirm-yes').textContent = actionLabel;
  return new Promise((resolve) => {
    const finish = (answer) => {
      dialog.removeEventListener('close', onClose);
      $('#confirm-yes').onclick = null;
      $('#confirm-no').onclick = null;
      if (dialog.open) dialog.close();
      resolve(answer);
    };
    const onClose = () => finish(false);
    dialog.addEventListener('close', onClose);
    $('#confirm-yes').onclick = () => finish(true);
    $('#confirm-no').onclick = () => finish(false);
    dialog.showModal();
    $('#confirm-no').focus();
  });
}

// ---- formatting ---------------------------------------------------------------

function ago(iso) {
  if (!iso) return '';
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 90) return 'just now';
  const minutes = seconds / 60;
  if (minutes < 60) return `${Math.round(minutes)} min ago`;
  const hours = minutes / 60;
  if (hours < 36) return `${Math.round(hours)} h ago`;
  const days = hours / 24;
  if (days < 60) return `${Math.round(days)} days ago`;
  return `${Math.round(days / 30)} months ago`;
}

function dateTime(iso) {
  if (!iso) return '';
  return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

function clock(iso) {
  const date = new Date(iso);
  const sameDay = date.toDateString() === new Date().toDateString();
  return date.toLocaleString(undefined, sameDay
    ? { timeStyle: 'short' }
    : { weekday: 'short', hour: 'numeric', minute: '2-digit' });
}

function fileSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function splitList(text) {
  return text.split(/[\n,]+/).map((item) => item.trim()).filter(Boolean);
}

// ---- board --------------------------------------------------------------------

function matchesFilter(job, query) {
  return `${job.title} ${job.company} ${job.location}`.toLowerCase().includes(query);
}

/** Totals from the last check, or null when none has finished. */
function lastCheckTotals() {
  const last = state.status && state.status.last_run;
  if (!last) return null;
  const sum = (field) => last.sources.reduce((total, source) => total + (source[field] || 0), 0);
  return { added: last.added, filtered: sum('filtered_out'), read: sum('fetched') };
}

/**
 * When a check found postings but the user's own settings left all of them out,
 * say so where they are looking, instead of leaving an unexplained empty column.
 */
function emptyNewNote() {
  const totals = lastCheckTotals();
  if (!totals || totals.added > 0 || totals.filtered === 0) return null;
  return h('div', { class: 'empty-note' },
    `The last check read ${totals.read.toLocaleString()} postings. `
    + `${totals.filtered.toLocaleString()} were remote but were left out by your role and region settings.`,
    h('div', null, h('button', { type: 'button', onclick: () => openSettings(false) }, 'Review settings')),
  );
}

function renderBoard() {
  const board = $('#board');
  const scroll = {};
  for (const body of board.querySelectorAll('.column-body')) {
    scroll[body.dataset.status] = body.scrollTop;
  }
  const query = state.filter.trim().toLowerCase();

  board.replaceChildren(...COLUMNS.map((column) => {
    const all = state.jobs.filter((job) => job.status === column.id);
    const jobs = query ? all.filter((job) => matchesFilter(job, query)) : all;
    const limit = state.shown[column.id] || PAGE_SIZE;

    const body = h('div', { class: 'column-body', 'data-status': column.id },
      jobs.slice(0, limit).map(renderCard),
      jobs.length > limit && h('button', {
        type: 'button',
        onclick: () => { state.shown[column.id] = limit + PAGE_SIZE; renderBoard(); },
      }, `Show ${Math.min(PAGE_SIZE, jobs.length - limit)} more`),
      jobs.length === 0 && h('p', { class: 'empty' }, query && all.length ? 'No cards match the filter.' : column.empty),
      column.id === 'new' && all.length === 0 && emptyNewNote(),
    );

    const section = h('section', { class: 'column', 'aria-label': column.label },
      h('header', { class: 'column-head' },
        h('h2', null, column.label),
        h('span', { class: 'count' }, query ? `${jobs.length} of ${all.length}` : String(all.length)),
      ),
      body,
    );

    section.addEventListener('dragover', (event) => {
      event.preventDefault();
      event.dataTransfer.dropEffect = 'move';
      section.classList.add('drop-target');
    });
    section.addEventListener('dragleave', (event) => {
      if (!section.contains(event.relatedTarget)) section.classList.remove('drop-target');
    });
    section.addEventListener('drop', (event) => {
      event.preventDefault();
      section.classList.remove('drop-target');
      const id = Number(event.dataTransfer.getData('text/plain'));
      if (id) moveJob(id, column.id);
    });
    return section;
  }));

  for (const body of board.querySelectorAll('.column-body')) {
    body.scrollTop = scroll[body.dataset.status] || 0;
  }
}

function renderCard(job) {
  const extras = [];
  if (job.note_count) extras.push(`${job.note_count} note${job.note_count === 1 ? '' : 's'}`);
  if (job.attachment_count) extras.push(`${job.attachment_count} file${job.attachment_count === 1 ? '' : 's'}`);
  const when = job.posted_at ? `posted ${ago(job.posted_at)}` : `found ${ago(job.found_at)}`;

  const card = h('article', {
    class: 'card',
    draggable: true,
    tabIndex: 0,
    'aria-label': `${job.title} at ${job.company}`,
  },
    h('h3', { class: 'card-title' }, job.title),
    h('p', { class: 'card-company' }, job.company || 'Company not stated'),
    h('p', { class: 'card-meta' }, [job.location, when].filter(Boolean).join(' · ')),
    h('div', { class: 'chips' },
      h('span', { class: `chip ${job.reason_kind}` }, WHY_LABELS[job.reason_kind] || 'Inclusive employer'),
      extras.map((text) => h('span', { class: 'chip' }, text)),
    ),
    h('div', { class: 'card-actions' },
      h('a', {
        class: 'button',
        href: job.url,
        target: '_blank',
        rel: 'noopener noreferrer',
        title: job.direct ? 'Opens the employer’s own listing' : `Opens the listing on ${job.source_label}`,
      }, 'Open posting ↗'),
      (QUICK_MOVES[job.status] || []).map(([target, label]) => h('button', {
        type: 'button',
        onclick: () => moveJob(job.id, target),
      }, label)),
    ),
  );

  card.addEventListener('click', (event) => {
    if (!event.target.closest('a, button')) openJob(job.id);
  });
  card.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && event.target === card) openJob(job.id);
  });
  card.addEventListener('dragstart', (event) => {
    event.dataTransfer.setData('text/plain', String(job.id));
    event.dataTransfer.effectAllowed = 'move';
    card.classList.add('dragging');
  });
  card.addEventListener('dragend', () => card.classList.remove('dragging'));
  return card;
}

async function loadJobs() {
  try {
    state.jobs = await api('GET', '/api/jobs');
    renderBoard();
  } catch (error) {
    fail(error);
  }
}

async function moveJob(id, status) {
  const job = state.jobs.find((candidate) => candidate.id === id);
  if (!job || job.status === status) return;
  // Show the move straight away; put it back if the server refuses.
  const previous = { status: job.status, changed: job.status_changed_at };
  job.status = status;
  job.status_changed_at = new Date().toISOString();
  state.jobs.sort((a, b) => (a.status_changed_at < b.status_changed_at ? 1 : -1));
  renderBoard();
  try {
    await api('PATCH', `/api/jobs/${id}`, { status });
  } catch (error) {
    job.status = previous.status;
    job.status_changed_at = previous.changed;
    renderBoard();
    fail(error);
  }
}

// ---- job detail ---------------------------------------------------------------

async function openJob(id) {
  try {
    state.openJob = await api('GET', `/api/jobs/${id}`);
  } catch (error) {
    fail(error);
    return;
  }
  renderJob();
  const dialog = $('#job-dialog');
  if (!dialog.open) dialog.showModal();
  // The address then points at this card, so it can be bookmarked or reopened.
  history.replaceState(null, '', `#job-${id}`);
}

async function reloadOpenJob() {
  if (!state.openJob) return;
  state.openJob = await api('GET', `/api/jobs/${state.openJob.id}`);
  renderJob();
  loadJobs();
}

function renderJob() {
  const job = state.openJob;
  const draft = $('#note-draft') ? $('#note-draft').value : '';

  const statusSelect = h('select', {
    id: 'job-status',
    onchange: async (event) => {
      job.status = event.target.value;
      await moveJob(job.id, event.target.value);
    },
  }, COLUMNS.map((column) => h('option', { value: column.id, selected: column.id === job.status }, column.label)));

  const description = h('div', { class: 'description' });
  if (job.description) {
    // The server strips scripts, styles, images and unsafe links before storing this.
    description.innerHTML = job.description;
  } else {
    description.append(h('p', { class: 'hint' }, 'This source did not include the description. Open the posting to read it.'));
  }

  $('#job-body').replaceChildren(
    h('div', { class: 'job-head' },
      h('div', null,
        h('h2', { id: 'job-title' }, job.title),
        h('p', { class: 'job-sub' }, [job.company, job.location, job.salary].filter(Boolean).join(' · ')),
      ),
      h('button', { type: 'button', onclick: () => $('#job-dialog').close() }, 'Close'),
    ),
    h('div', { class: 'job-bar' },
      h('a', { class: 'button primary', href: job.url, target: '_blank', rel: 'noopener noreferrer' },
        job.direct ? 'Open the employer’s listing ↗' : `Open the listing on ${job.source_label} ↗`),
      h('label', null, 'Column', statusSelect),
      h('span', { class: 'hint' }, [
        job.posted_at && `Posted ${dateTime(job.posted_at)}`,
        `Found ${dateTime(job.found_at)} via ${job.source_label}`,
      ].filter(Boolean).join(' · ')),
    ),
    h('div', { class: 'why' },
      h('strong', null, 'Why this is on your board'),
      job.reason,
      job.evidence_url && [' ', h('a', { href: job.evidence_url, target: '_blank', rel: 'noopener noreferrer' }, 'Source ↗')],
    ),
    h('div', { class: 'job-grid' },
      h('div', null, renderNotes(job, draft), renderFiles(job)),
      h('section', null, h('h3', null, 'Description'), description),
    ),
  );
}

function renderNotes(job, draft) {
  const input = h('textarea', {
    id: 'note-draft',
    rows: 3,
    placeholder: 'Write a note…',
    'aria-label': 'New note',
    value: draft,
    onkeydown: (event) => {
      if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) addNote();
    },
  });
  const addNote = async () => {
    const body = input.value.trim();
    if (!body) return;
    try {
      await api('POST', `/api/jobs/${job.id}/notes`, { body });
      input.value = '';
      await reloadOpenJob();
    } catch (error) {
      fail(error);
    }
  };
  return h('section', null,
    h('h3', null, 'Notes'),
    h('div', { class: 'note-form' }, input, h('button', { type: 'button', onclick: addNote }, 'Add note')),
    job.notes.map((note) => renderNote(note)),
  );
}

function renderNote(note) {
  const view = h('div', { class: 'note' });
  const show = () => {
    const edited = note.updated_at !== note.created_at;
    view.replaceChildren(
      h('div', { class: 'note-body' }, note.body),
      h('div', { class: 'row-meta' },
        h('span', null, dateTime(note.created_at) + (edited ? ' (edited)' : '')),
        h('span', { class: 'row-actions' },
          h('button', { type: 'button', onclick: edit }, 'Edit'),
          h('button', {
            type: 'button',
            class: 'danger',
            onclick: async () => {
              if (!await confirmAction('Delete this note?', 'Delete note')) return;
              try {
                await api('DELETE', `/api/notes/${note.id}`);
                await reloadOpenJob();
              } catch (error) {
                fail(error);
              }
            },
          }, 'Delete'),
        ),
      ),
    );
  };
  const edit = () => {
    const input = h('textarea', { rows: 4, value: note.body, 'aria-label': 'Edit note' });
    view.replaceChildren(
      input,
      h('div', { class: 'row-meta' },
        h('span', null, ''),
        h('span', { class: 'row-actions' },
          h('button', { type: 'button', onclick: show }, 'Cancel'),
          h('button', {
            type: 'button',
            onclick: async () => {
              const body = input.value.trim();
              if (!body) return;
              try {
                await api('PUT', `/api/notes/${note.id}`, { body });
                await reloadOpenJob();
              } catch (error) {
                fail(error);
              }
            },
          }, 'Save'),
        ),
      ),
    );
    input.focus();
  };
  show();
  return view;
}

function renderFiles(job) {
  const picker = h('input', { type: 'file', multiple: true, hidden: true });
  const upload = async (files) => {
    if (!files.length) return;
    try {
      for (const file of files) {
        await api('POST', `/api/jobs/${job.id}/attachments`, await file.arrayBuffer(), {
          'Content-Type': 'application/octet-stream',
          'X-File-Name': encodeURIComponent(file.name),
        });
      }
    } catch (error) {
      fail(error);
    }
    try {
      await reloadOpenJob();
    } catch (error) {
      fail(error);
    }
  };
  picker.addEventListener('change', () => upload([...picker.files]));

  const zone = h('div', { class: 'dropzone' },
    h('button', { type: 'button', onclick: () => picker.click() }, 'Add files'),
    ' or drop them here',
    picker,
  );
  zone.addEventListener('dragover', (event) => {
    event.preventDefault();
    zone.classList.add('drop-target');
  });
  zone.addEventListener('dragleave', () => zone.classList.remove('drop-target'));
  zone.addEventListener('drop', (event) => {
    event.preventDefault();
    zone.classList.remove('drop-target');
    upload([...event.dataTransfer.files]);
  });

  return h('section', null,
    h('h3', null, 'Attachments'),
    zone,
    job.attachments.map((file) => h('div', { class: 'file' },
      h('button', {
        type: 'button',
        class: 'file-name',
        title: 'Open this file',
        onclick: () => api('POST', `/api/attachments/${file.id}/open`).catch(fail),
      }, file.filename),
      h('span', { class: 'row-actions' },
        h('span', { class: 'file-size' }, fileSize(file.size)),
        h('button', {
          type: 'button',
          onclick: () => api('POST', `/api/attachments/${file.id}/reveal`).catch(fail),
        }, 'Show in Finder'),
        h('button', {
          type: 'button',
          class: 'danger',
          onclick: async () => {
            const sure = await confirmAction(
              `Remove “${file.filename}”? The copy stored with this card is deleted. Your original file is not touched.`,
              'Remove file',
            );
            if (!sure) return;
            try {
              await api('DELETE', `/api/attachments/${file.id}`);
              await reloadOpenJob();
            } catch (error) {
              fail(error);
            }
          },
        }, 'Remove'),
      ),
    )),
  );
}

// ---- status and checks ----------------------------------------------------------

function renderStatus() {
  const status = state.status;
  const button = $('#status-button');
  const refresh = $('#refresh');
  if (!status) return;
  refresh.disabled = status.scanning;
  if (status.scanning) {
    button.textContent = 'Checking for jobs…';
    return;
  }
  const last = status.last_run;
  if (!last) {
    button.textContent = 'Not checked yet';
    return;
  }
  const failed = last.sources.filter((source) => source.error).length;
  const parts = [`Checked ${ago(last.finished_at)}`];
  if (failed) parts.push(`${failed} source${failed === 1 ? '' : 's'} failed`);
  parts.push(`next ${clock(status.next_check_at)}`);
  button.textContent = parts.join(' · ');
}

async function pollStatus() {
  try {
    state.status = await api('GET', '/api/status');
  } catch (_) {
    $('#status-button').textContent = 'The app is not running';
    return;
  }
  renderStatus();
  if (state.wasScanning && !state.status.scanning) {
    const added = state.status.last_run ? state.status.last_run.added : 0;
    const totals = lastCheckTotals();
    if (added) toast(`${added} new job${added === 1 ? '' : 's'} added.`);
    else if (totals && totals.filtered) toast(`Check finished. Nothing new matched your settings (${totals.filtered.toLocaleString()} remote postings left out).`);
    else toast('Check finished. Nothing new.');
    loadJobs();
  }
  state.wasScanning = state.status.scanning;
}

async function checkNow() {
  try {
    await api('POST', '/api/refresh');
    state.wasScanning = true;
    if (state.status) state.status.scanning = true;
    renderStatus();
  } catch (error) {
    fail(error);
  }
}

function showLastRun() {
  const status = state.status;
  const body = $('#run-body');
  const last = status && status.last_run;
  if (!last) {
    body.replaceChildren(h('p', { class: 'run-summary' },
      status && status.scanning ? 'The first check is running now.' : 'No check has finished yet.'));
  } else {
    const column = (label, title) => h('th', { title }, label);
    body.replaceChildren(
      h('p', { class: 'run-summary' },
        `Finished ${dateTime(last.finished_at)}. ${last.added} new job${last.added === 1 ? '' : 's'} added. `
        + `Checks run every ${status.interval_hours} hours; the next is ${clock(status.next_check_at)}.`),
      h('div', { class: 'table-scroll' },
        h('table', { class: 'run-table' },
          h('thead', null, h('tr', null,
            h('th', null, 'Source'),
            column('Read', 'Postings the source returned'),
            column('Added', 'New cards'),
            column('Seen before', 'Already on the board, or already judged'),
            column('Not remote', 'Dropped: the listing did not clearly say remote'),
            column('Not inclusive', 'Dropped: nothing showed the employer welcomes neurodivergent or disabled applicants'),
            column('Filtered', 'Dropped by your role and region settings'),
            column('Duplicate', 'A board copy of a job already on the board'),
            column('Later', 'Left for the next check to keep requests polite'),
          )),
          h('tbody', null, last.sources.map((source) => (source.error
            ? h('tr', null, h('td', null, source.label), h('td', { class: 'error', colSpan: 8 }, source.error))
            : h('tr', null,
              h('td', null, source.label),
              [source.fetched, source.added, source.already_known, source.not_remote,
                source.not_inclusive, source.filtered_out, source.duplicate, source.deferred]
                .map((value) => h('td', null, String(value))),
            )))),
        ),
      ),
    );
  }
  $('#run-dialog').showModal();
}

// ---- settings -------------------------------------------------------------------

async function openSettings(firstRun) {
  let settings;
  try {
    settings = await api('GET', '/api/settings');
  } catch (error) {
    fail(error);
    return;
  }
  $('#settings-intro').hidden = !firstRun;
  $('#settings-error').hidden = true;
  $('#set-roles').value = settings.role_keywords.join(', ');
  $('#set-exclude').value = settings.exclude_keywords.join(', ');
  $('#set-regions').value = settings.regions.join(', ');
  if (state.status) {
    $('#path-employers').textContent = state.status.employers_file;
    $('#path-data').textContent = state.status.data_dir;
    $('#path-log').textContent = state.status.log_file;
  }
  $('#settings-dialog').showModal();
}

async function saveSettings(event) {
  event.preventDefault();
  try {
    await api('PUT', '/api/settings', {
      role_keywords: splitList($('#set-roles').value),
      exclude_keywords: splitList($('#set-exclude').value),
      regions: splitList($('#set-regions').value),
    });
  } catch (error) {
    // Shown in the dialog: a message behind it would go unseen.
    const box = $('#settings-error');
    box.textContent = error.message || String(error);
    box.hidden = false;
    box.scrollIntoView({ block: 'nearest' });
    return;
  }
  $('#settings-dialog').close();
  toast('Saved. Checking for jobs with the new settings…');
  checkNow();
}

// ---- start ----------------------------------------------------------------------

async function start() {
  $('#search').addEventListener('input', (event) => {
    state.filter = event.target.value;
    renderBoard();
  });
  $('#refresh').addEventListener('click', checkNow);
  $('#status-button').addEventListener('click', showLastRun);
  $('#open-settings').addEventListener('click', () => openSettings(false));
  $('#settings-cancel').addEventListener('click', () => $('#settings-dialog').close());
  $('#settings-form').addEventListener('submit', saveSettings);
  $('#run-close').addEventListener('click', () => $('#run-dialog').close());
  $('#job-dialog').addEventListener('close', () => {
    state.openJob = null;
    history.replaceState(null, '', location.pathname);
  });
  // Clicking the dimmed area around a dialog closes it.
  for (const dialog of document.querySelectorAll('dialog')) {
    dialog.addEventListener('click', (event) => {
      if (event.target === dialog) dialog.close();
    });
  }

  renderBoard();
  await Promise.all([loadJobs(), pollStatus()]);
  renderBoard();
  setInterval(pollStatus, 5000);
  setInterval(loadJobs, 120000);
  window.addEventListener('focus', loadJobs);
  // A hidden window is slowed down by the system; catch up the moment it is shown.
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) {
      pollStatus();
      loadJobs();
    }
  });

  const linked = /^#job-(\d+)$/.exec(location.hash);
  if (linked) openJob(Number(linked[1]));

  try {
    const settings = await api('GET', '/api/settings');
    if (!settings.configured && !linked) openSettings(true);
  } catch (_) { /* the status line already reports it */ }
}

start();
