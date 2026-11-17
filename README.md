# JobSearch

A desktop app for macOS that looks for **remote** jobs at employers that actively
hire disabled and neurodivergent people (ADHD, autism and others), and keeps them on
a kanban board on your own Mac.

- Checks for jobs twice a day, in the background.
- Columns: **New**, **Evaluating**, **Applied to**, **Pass**, and any others you add.
- Each card has notes, file attachments, a link to the posting, and the reason it is
  on your board.
- Only ever shows postings whose listing says the job is remote.
- Everything is stored locally. Nothing about you is sent anywhere.

## Run it

```sh
scripts/bundle.sh --install     # builds the app and copies it into /Applications
open /Applications/JobSearch.app
```

The app opens in its own window. The first time, it asks which roles you want and
where you can work from, then runs its first check (about two minutes).

Closing the window does not quit the app: it keeps checking for jobs in the
background. Click its Dock icon, or the briefcase in the menu bar, to bring the
window back. Use **Quit** to stop it.

The briefcase in the menu bar shows how many cards are waiting in **New**. Its menu
has:

- **Open Job Board**
- **Check for Jobs Now**
- **Start at Login** — off until you turn it on. It starts the app in the background,
  without opening the window. If you move the app afterwards, turn this off and on
  again so it points at the new location.
- **Quit**

Links to postings open in your browser. Attachments open in whichever app macOS uses
for that kind of file.

## Where jobs come from

| Source | What it is | When a job becomes a card | Card label |
| --- | --- | --- | --- |
| Neurodiversity @ Work Employer Roundtable | Employers on Disability:IN's roundtable, "committed to hiring and supporting neurodivergent employees". 18 of their career sites are read directly, among them Salesforce, DXC, Travelers, Boeing, Pfizer, Bank of America and General Dynamics IT | Every remote job | Neurodiversity hiring program |
| Disability Equality Index | 48 employers named a "Best Place to Work for Disability Inclusion" on the 2024 index, among them Leidos, Accenture, AT&T, Cigna, General Motors and GitLab | Every remote job | Disability inclusion index |
| Hire Autism | The Organization for Autism Research's job board, where employers post to reach autistic candidates | Every remote listing | Autism job board |
| Other employers' career sites | Automattic, Zapier and others listed in `employers.toml` | Only when the posting's own text welcomes neurodivergent or disabled applicants | Says so in the posting |
| Remote job boards | Remotive, Remote OK, Jobicy, Himalayas, We Work Remotely, Working Nomads | Same: only when the posting says so, or the employer is on one of the lists above | (as above) |

Each card says which of these applied, quotes the sentence from the posting when
there is one, and links to the evidence so you can check it. The standard
equal-opportunity paragraph that appears on almost every posting does not count.

The two employer lists are not equally strong:

- The **roundtable** is about neurodivergent hiring specifically. Its roster was read
  from Disability:IN's page in November 2026.
- The **index** measures disability inclusion across a whole company. It does not
  mean the employer runs a neurodiversity hiring program. The scores are from 2024,
  the most recent year for which a list could be found; Disability:IN no longer
  publishes one.

Jobs read from an employer's career site link straight to the employer's own
listing. Jobs found on a board link to the board's listing.

### LinkedIn

LinkedIn is not read. Tested on 2026-11-16, its public job search ignored the remote
filter and returned on-site jobs, and the link to the employer's own listing is only
shown after signing in. Signing the app in to your LinkedIn account would put that
account at risk of being restricted, which is a bad trade while job hunting. Reading
employers' career sites directly gets the same postings at their source, with a
dependable remote label.

### The remote-only rule

A posting is shown only if:

1. the source marks it remote in a structured field, or the board lists remote jobs
   only, or the location or title plainly says remote; **and**
2. nothing in the title or location says hybrid, on-site or in-office; **and**
3. the description does not say this job is hybrid or on-site (for example "this is
   a hybrid role", "three days per week in the office"); **and**
4. the employer's own workplace label, where it has one, says remote and nothing
   else. "Hybrid or 100% Remote", "Remote/Hybrid" and "Flexible" are all dropped,
   whatever the title says.

When a listing is ambiguous it is dropped. That means some genuinely remote jobs
will be missed. An employer can also label a job remote and still mean something
else by it, so read the posting before applying.

## Columns

The board starts with **New**, **Evaluating**, **Applied to** and **Pass**. Those four
stay as they are. **Add column** in the top bar adds one of your own, for example
"Interviewing" or "Offer". It is placed just before Pass.

A column you added has a **⋯** button in its heading, to rename it, move it left or
right, or delete it. Nothing can go in front of New. Deleting a column never deletes
cards: they move to the column on its left, and the app says so before you confirm.

Cards move between columns by dragging, with the **Column** menu inside a card, or
with the buttons on the card: one for the next column along, and **Pass**.

## Settings

**Settings** in the app:

- **Show jobs with these in the title** — the roles you want. A job is shown when
  its title contains every word of one entry, in any order. Plurals and common
  abbreviations are treated as the same word, so "system engineer" also finds
  "Sr. Systems Engineer" and "system administrator" finds "Linux System Admin".
- **Hide jobs with these in the title** — only for titles you do not want. The app
  refuses to save a hide entry that would hide every job matching one of your roles.
- **Hide jobs that mention these anywhere in the posting** — phrases you do not want
  to see in the title or the description, for example "security clearance" or
  "polygraph". See below.
- **Where I can work from** — for remote jobs limited to certain countries. Jobs open
  worldwide, or that name no country, are always kept. Listing the United States also
  matches postings that name only a US state, or a region such as "NAMER".

If a check finds postings but your settings leave all of them out, the New column
says so and links to Settings.

Changes to roles and places apply from the next check, and jobs already on the board
stay there. The two hide lists also apply to the board straight away, as described
next.

### Hiding by phrase

A phrase matches when the posting has the same words in the same order, as whole
words. Capitals and punctuation do not matter: "TS/SCI" also finds "TS / SCI", and
"clearance" does not find "clearances".

A mention that says the thing does not apply is not counted. Many postings carry a
line such as "Security Clearance: None", "Clearance Level Must Currently Possess:
None" or "no security clearance required"; hiding "clearance" leaves those on the
board and hides the jobs that ask for one. The rule is narrow on purpose. A
requirement worded with a negative ("candidates without an active clearance will not
be considered") still counts as a mention.

It is a text match, not a reading of the posting, so check what it hides:

- When you save, cards already in **New** that match are hidden at once, and the app
  says how many. Cards you have moved, or added a note or a file to, are never
  hidden.
- Nothing is deleted. **Settings** lists every hidden card with the entry that hid
  it. Remove the entry and its cards come back; or open a hidden card from that list
  and change its column to keep just that one.
- The New column shows how many cards are hidden.

### Adding employers

The employer list is a text file, created on first launch:

    ~/Library/Application Support/JobSearch/employers.toml

It explains its own format. Add an employer with `mode = "signal"` to have its remote
jobs checked for inclusive language, or `mode = "program"` if you know it qualifies
and want all of its remote jobs. Greenhouse, Lever, Ashby and Workday career sites
are supported.

Until you edit that file, the app keeps it in step with its built-in list, so an
update can add employers. Once you change it, it is left alone.

## Your data

Everything lives in `~/Library/Application Support/JobSearch/`:

- `jobs.sqlite3` — cards, notes, columns, settings
- `attachments/` — files you attach to cards, under their own names
- `employers.toml` — the employer list
- `jobsearch.log` — what each check did

Back up or move that folder to keep your board. The app opens no network port: its
window talks to the rest of the app inside the same process.

To see why a particular job is not on your board, run a check with
`JOBSEARCH_DEBUG=1`; every dropped posting is then written to the log with the
reason.

## Development

```sh
cargo test                       # unit tests for the filters, parsers and handlers
cargo run                        # the desktop app
cargo run -- --scan-once         # run one check and print what each source returned
JOBSEARCH_DATA_DIR=/tmp/js cargo run -- --serve 4800   # no window: the board at http://127.0.0.1:4800/
scripts/make-icon.sh             # redraw the app icon after editing scripts/icon.swift
```

`--serve` exists so the board can be driven by a browser automation tool. It is not
used by the app itself.

To run a script inside the app's real window, set `JOBSEARCH_UI_TEST` to the
script's path. Whatever the script sends with `window.ipc.postMessage` is appended
to the file named by `JOBSEARCH_UI_TEST_OUT`. While that is set, links and
attachments are logged, not opened.

Layout:

- `src/sources/` — one fetcher per source (`ats.rs` for career sites, `boards.rs` for job boards)
- `src/filter.rs` — the remote, inclusion, role and region rules
- `src/scan.rs` — one check: fetch, filter, add cards
- `src/scheduler.rs` — twice a day, plus on request
- `src/server.rs`, `assets/` — the board and its request handlers
- `src/desktop.rs` — the window, the menu-bar icon and the menus
- `data/employers.toml` — the built-in employer list

## Not built yet

- Applying automatically.
