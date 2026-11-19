'use strict';

// The columns every board starts with. The app replaces this list with the real
// one, which also holds the columns you have added.
const BUILTIN_COLUMNS = [
  { id: 'new', label: 'New', builtin: true },
  { id: 'evaluating', label: 'Evaluating', builtin: true },
  { id: 'applied', label: 'Applied to', builtin: true },
  { id: 'pass', label: 'Pass', builtin: true },
];

const EMPTY_TEXT = {
  new: 'Nothing new right now.',
  evaluating: 'Drag a card here while you look into it.',
  applied: 'Drag a card here once you have applied.',
  pass: 'Jobs you pass on stay here and are never shown as new again.',
};

// What a card's one-click button calls a move into a built-in column. A column you
// added is called by its name.
const MOVE_VERBS = { evaluating: 'Evaluate', applied: 'Applied' };

const WHY_LABELS = {
  program: 'Neurodiversity hiring program',
  index: 'Disability inclusion index',
  board: 'Autism job board',
  signal: 'Says so in the posting',
};

// The kind of a card that claims nothing about the employer. There are only any
// while Settings is not keeping the board to inclusive employers.
const NO_INCLUSION_CLAIM = 'none';

// Where a job is done, as far as its posting shows.
const WORKPLACE_LABELS = {
  remote: 'Remote',
  hybrid: 'Hybrid',
  onsite: 'On-site',
  unstated: 'Workplace not stated',
};

const PAGE_SIZE = 100;

const state = {
  columns: BUILTIN_COLUMNS,
  jobs: [],
  filter: '',
  shown: {},
  status: null,
  settings: null,
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
    + `${totals.filtered.toLocaleString()} were the kind of job you asked for but were left out by your other settings.`,
    h('div', null, h('button', { type: 'button', onclick: () => openSettings(false) }, 'Review settings')),
  );
}

/** The one-click moves offered on a card: on to the next column, and Pass. */
function quickMoves(status) {
  if (status === 'pass') return [['new', 'Restore']];
  const at = state.columns.findIndex((column) => column.id === status);
  const next = at >= 0 ? state.columns[at + 1] : null;
  const moves = [];
  if (next && next.id !== 'pass') moves.push([next.id, MOVE_VERBS[next.id] || next.label]);
  moves.push(['pass', 'Pass']);
  return moves;
}

function columnLabel(id) {
  const column = state.columns.find((candidate) => candidate.id === id);
  return column ? column.label : id;
}

/**
 * What a card says about where the job is done, or null when it need not say. On a
 * board of nothing but remote jobs, which is how every board starts, saying
 * "Remote" on each card would tell the reader nothing.
 */
function workplaceLabel(job) {
  if (job.workplace === 'remote' && !boardIsMixed()) return null;
  return WORKPLACE_LABELS[job.workplace] || null;
}

// Worked out once for each list of jobs the board loads, not once for each card.
const mixedBoard = { jobs: null, mixed: false };
function boardIsMixed() {
  if (mixedBoard.jobs !== state.jobs) {
    mixedBoard.jobs = state.jobs;
    mixedBoard.mixed = state.jobs.some((job) => job.workplace !== 'remote');
  }
  return mixedBoard.mixed;
}

function renderBoard() {
  const board = $('#board');
  const scroll = {};
  for (const body of board.querySelectorAll('.column-body')) {
    scroll[body.dataset.status] = body.scrollTop;
  }
  const query = state.filter.trim().toLowerCase();

  board.replaceChildren(...state.columns.map((column) => {
    const all = state.jobs.filter((job) => job.status === column.id);
    const jobs = query ? all.filter((job) => matchesFilter(job, query)) : all;
    const limit = state.shown[column.id] || PAGE_SIZE;

    const hiddenCount = column.id === 'new' && state.status ? state.status.hidden_cards : 0;
    const body = h('div', { class: 'column-body', 'data-status': column.id },
      hiddenCount > 0 && h('button', {
        type: 'button',
        class: 'quiet hidden-note',
        title: 'Show which cards are hidden, and why',
        onclick: () => openSettings(false),
      }, `${hiddenCount} more hidden by your hide lists`),
      jobs.slice(0, limit).map(renderCard),
      jobs.length > limit && h('button', {
        type: 'button',
        onclick: () => { state.shown[column.id] = limit + PAGE_SIZE; renderBoard(); },
      }, `Show ${Math.min(PAGE_SIZE, jobs.length - limit)} more`),
      jobs.length === 0 && h('p', { class: 'empty' },
        query && all.length ? 'No cards match the filter.' : (EMPTY_TEXT[column.id] || 'Drag a card here.')),
      column.id === 'new' && all.length === 0 && emptyNewNote(),
    );

    const section = h('section', { class: 'column', 'aria-label': column.label },
      h('header', { class: 'column-head' },
        h('h2', null, column.label),
        h('span', { class: 'column-tools' },
          h('span', { class: 'count' }, query ? `${jobs.length} of ${all.length}` : String(all.length)),
          !column.builtin && h('button', {
            type: 'button',
            class: 'quiet column-edit',
            title: 'Rename, move or delete this column',
            'aria-label': `Edit the ${column.label} column`,
            onclick: () => openColumnDialog(column),
          }, '⋯'),
        ),
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
  const chips = extras.map((text) => h('span', { class: 'chip' }, text));
  const workplace = workplaceLabel(job);
  if (workplace) chips.unshift(h('span', { class: 'chip workplace' }, workplace));
  if (job.reason_kind !== NO_INCLUSION_CLAIM) {
    chips.unshift(h('span', { class: `chip ${job.reason_kind}` }, WHY_LABELS[job.reason_kind] || 'Inclusive employer'));
  }

  const card = h('article', {
    class: 'card',
    draggable: true,
    tabIndex: 0,
    'aria-label': `${job.title} at ${job.company}`,
  },
    h('h3', { class: 'card-title' }, job.title),
    h('p', { class: 'card-company' }, job.company || 'Company not stated'),
    h('p', { class: 'card-meta' }, [job.location, when].filter(Boolean).join(' · ')),
    chips.length > 0 && h('div', { class: 'chips' }, chips),
    h('div', { class: 'card-actions' },
      h('a', {
        class: 'button',
        href: job.url,
        target: '_blank',
        rel: 'noopener noreferrer',
        title: job.direct ? 'Opens the employer’s own listing' : `Opens the listing on ${job.source_label}`,
      }, 'Open posting ↗'),
      quickMoves(job.status).map(([target, label]) => h('button', {
        type: 'button',
        title: `Move to ${columnLabel(target)}`,
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
  if (!job) {
    // A card the hide lists keep off the board, opened from Settings. Moving it
    // brings it back.
    try {
      await api('PATCH', `/api/jobs/${id}`, { status });
      await loadJobs();
      if ($('#settings-dialog').open) showHiddenCards();
    } catch (error) {
      fail(error);
    }
    return;
  }
  if (job.status === status) return;
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

// ---- columns --------------------------------------------------------------------

function setColumns(columns) {
  state.columns = columns;
  renderBoard();
  if (state.openJob) renderJob();
}

async function loadColumns() {
  try {
    setColumns(await api('GET', '/api/columns'));
  } catch (error) {
    fail(error);
  }
}

// The id of the column being edited, or null while adding one.
let editingColumn = null;

function columnError(error) {
  const box = $('#column-error');
  box.textContent = error.message || String(error);
  box.hidden = false;
  // Back to the name, which is what needs changing.
  $('#column-name').focus();
}

/** Says where the column being edited sits, and which ways it can still move. */
function describeColumnPosition() {
  const at = state.columns.findIndex((column) => column.id === editingColumn);
  if (at < 0) return;
  const left = state.columns[at - 1];
  const right = state.columns[at + 1];
  // New always stays first.
  $('#column-left').disabled = at <= 1;
  $('#column-right').disabled = !right;
  $('#column-where').textContent = right
    ? `Between ${left.label} and ${right.label}`
    : `Last, after ${left.label}`;
}

function openColumnDialog(column) {
  editingColumn = column ? column.id : null;
  $('#column-title').textContent = column ? 'Edit column' : 'Add a column';
  $('#column-name').value = column ? column.label : '';
  $('#column-hint').hidden = Boolean(column);
  $('#column-position').hidden = !column;
  $('#column-delete').hidden = !column;
  $('#column-save').textContent = column ? 'Save' : 'Add column';
  $('#column-error').hidden = true;
  describeColumnPosition();
  $('#column-dialog').showModal();
  $('#column-name').focus();
  $('#column-name').select();
}

async function saveColumn(event) {
  event.preventDefault();
  const label = $('#column-name').value.trim();
  try {
    if (editingColumn === null) {
      const before = new Set(state.columns.map((column) => column.id));
      setColumns(await api('POST', '/api/columns', { label }));
      // Bring the new column into view on a board too wide for the window.
      const added = state.columns.find((column) => !before.has(column.id));
      const body = added && document.querySelector(`.column-body[data-status="${added.id}"]`);
      if (body) body.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    } else if (label !== columnLabel(editingColumn)) {
      setColumns(await api('PATCH', `/api/columns/${editingColumn}`, { label }));
    }
  } catch (error) {
    columnError(error);
    return;
  }
  $('#column-dialog').close();
}

async function moveColumn(direction) {
  $('#column-error').hidden = true;
  try {
    setColumns(await api('PATCH', `/api/columns/${editingColumn}`, { move: direction }));
    describeColumnPosition();
  } catch (error) {
    columnError(error);
  }
}

async function deleteColumn() {
  const at = state.columns.findIndex((column) => column.id === editingColumn);
  if (at < 1) return;
  const column = state.columns[at];
  const destination = state.columns[at - 1];
  const count = state.jobs.filter((job) => job.status === column.id).length;
  const consequence = count === 0
    ? 'It has no cards.'
    : `Its ${count === 1 ? 'card moves' : `${count} cards move`} to ${destination.label}.`;
  if (!await confirmAction(`Delete the column “${column.label}”? ${consequence}`, 'Delete column')) return;
  try {
    const result = await api('DELETE', `/api/columns/${column.id}`);
    setColumns(result.columns);
    await loadJobs();
    if (result.moved) toast(`${result.moved} card${result.moved === 1 ? '' : 's'} moved to ${destination.label}.`);
  } catch (error) {
    columnError(error);
    return;
  }
  $('#column-dialog').close();
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
  }, state.columns.map((column) => h('option', { value: column.id, selected: column.id === job.status }, column.label)));

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
        h('p', { class: 'job-sub' }, [job.company, job.location, workplaceLabel(job), job.salary].filter(Boolean).join(' · ')),
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
  const hiddenBefore = state.status ? state.status.hidden_cards : 0;
  try {
    state.status = await api('GET', '/api/status');
  } catch (_) {
    $('#status-button').textContent = 'The app is not running';
    return;
  }
  renderStatus();
  if (state.status.hidden_cards !== hiddenBefore) renderBoard();
  if (state.wasScanning && !state.status.scanning) {
    const added = state.status.last_run ? state.status.last_run.added : 0;
    const totals = lastCheckTotals();
    if (added) toast(`${added} new job${added === 1 ? '' : 's'} added.`);
    else if (totals && totals.filtered) toast(`Check finished. Nothing new matched your settings (${totals.filtered.toLocaleString()} postings left out).`);
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
    const wanted = state.settings ? state.settings.workplaces : null;
    const remoteOnly = !wanted || (wanted.remote && !wanted.hybrid && !wanted.onsite);
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
            remoteOnly
              ? column('Not remote', 'Dropped: the listing did not clearly say remote')
              : column('Other workplace', 'Dropped: not one of the kinds of job (remote, hybrid, on-site) that Settings asks for'),
            column('Not inclusive', 'Dropped: nothing showed the employer welcomes neurodivergent or disabled applicants. Always 0 while Settings shows jobs from any employer'),
            column('Filtered', 'Dropped by your settings: role, region or a hide list'),
            column('Duplicate', 'Another posting of a job whose card you have moved out of New and Evaluating, or a job board’s copy of a job already on the board'),
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
  $('#set-terms').value = settings.exclude_terms.join(', ');
  $('#set-regions').value = settings.regions.join(', ');
  $('#set-remote').checked = settings.workplaces.remote;
  $('#set-hybrid').checked = settings.workplaces.hybrid;
  $('#set-onsite').checked = settings.workplaces.onsite;
  $('#set-nearby').value = settings.nearby.join(', ');
  showNearbyField();
  $('#set-inclusive').checked = settings.inclusive_only;
  showHiddenCards();
  if (state.status) {
    $('#path-employers').textContent = state.status.employers_file;
    $('#path-data').textContent = state.status.data_dir;
    $('#path-log').textContent = state.status.log_file;
  }
  $('#settings-dialog').showModal();
}

/** The places to travel to only matter once hybrid or on-site jobs are asked for. */
function showNearbyField() {
  $('#nearby-field').hidden = !($('#set-hybrid').checked || $('#set-onsite').checked);
}

/** Lists, in Settings, the cards the hide lists are keeping off the board. */
async function showHiddenCards() {
  const details = $('#hidden-details');
  details.hidden = true;
  let hidden;
  try {
    hidden = await api('GET', '/api/hidden');
  } catch (_) {
    return;
  }
  if (!hidden.length) return;
  $('#hidden-summary').textContent = hidden.length === 1
    ? '1 card on your board is hidden by these lists'
    : `${hidden.length} cards on your board are hidden by these lists`;
  details.open = false;
  $('#hidden-list').replaceChildren(...hidden.map((card) => h('li', null,
    h('button', { type: 'button', title: 'Open this card', onclick: () => openJob(card.id) },
      [card.title, card.company].filter(Boolean).join(' · ')),
    h('span', { class: 'hidden-why' }, `“${card.hidden_by}”`),
  )));
  details.hidden = false;
}

async function saveSettings(event) {
  event.preventDefault();
  let saved;
  try {
    saved = await api('PUT', '/api/settings', {
      role_keywords: splitList($('#set-roles').value),
      exclude_keywords: splitList($('#set-exclude').value),
      exclude_terms: splitList($('#set-terms').value),
      regions: splitList($('#set-regions').value),
      workplaces: {
        remote: $('#set-remote').checked,
        hybrid: $('#set-hybrid').checked,
        onsite: $('#set-onsite').checked,
      },
      nearby: splitList($('#set-nearby').value),
      inclusive_only: $('#set-inclusive').checked,
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
  state.settings = saved;
  const hidden = saved.hidden_cards;
  toast(hidden
    ? `Saved. ${hidden} card${hidden === 1 ? ' is' : 's are'} now hidden by your hide lists. Checking for jobs…`
    : 'Saved. Checking for jobs with the new settings…');
  // The hide lists apply to the cards already here, straight away.
  if (state.status) state.status.hidden_cards = hidden;
  loadJobs();
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
  $('#set-hybrid').addEventListener('change', showNearbyField);
  $('#set-onsite').addEventListener('change', showNearbyField);
  $('#add-column').addEventListener('click', () => openColumnDialog(null));
  $('#column-form').addEventListener('submit', saveColumn);
  $('#column-cancel').addEventListener('click', () => $('#column-dialog').close());
  $('#column-left').addEventListener('click', () => moveColumn('left'));
  $('#column-right').addEventListener('click', () => moveColumn('right'));
  $('#column-delete').addEventListener('click', deleteColumn);
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
  await Promise.all([loadColumns(), loadJobs(), pollStatus()]);
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
    state.settings = settings;
    if (!settings.configured && !linked) openSettings(true);
  } catch (_) { /* the status line already reports it */ }
}

start();
