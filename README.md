# JobSearch

A desktop app for macOS that looks for jobs at employers that actively hire disabled
and neurodivergent people (ADHD, autism and others), and keeps them on a kanban board
on your own Mac. It starts out showing **remote** jobs only.

Both of those are settings. You can add hybrid and on-site jobs, and you can turn off
the inclusive-employers rule to see jobs from any employer.

- Checks for jobs twice a day, in the background.
- Columns: **New**, **Evaluating**, **Applied to**, **Pass**, and any others you add.
- Each card has notes, file attachments, a link to the posting, and the reason it is
  on your board.
- Calls a job remote only when its listing says so. Hybrid and on-site jobs stay off
  the board until you ask for them.
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

## Giving the app to other people

`scripts/bundle.sh` signs the app for the Mac it was built on. Copied to another Mac,
macOS will refuse to open it. `scripts/bundle.sh --release` makes a copy that opens
anywhere without a warning: it signs the app with your Apple Developer ID, has Apple
notarize it, and packs it as `dist/JobSearch-<version>.dmg`.

That needs two things set up once, both tied to your Apple account:

1. **A Developer ID Application certificate.** It comes with membership of the Apple
   Developer Program, which is paid yearly. At developer.apple.com, under
   Certificates, IDs & Profiles, add a certificate of the kind *Developer ID
   Application*. It asks for a certificate signing request, which Keychain Access
   makes (Certificate Assistant, then Request a Certificate From a Certificate
   Authority, saved to disk). Download the certificate and double-click it. Only a
   team's account holder can create this kind. When it is in place,
   `security find-identity -v -p codesigning` lists it.
2. **Notary credentials.** Create an app-specific password at account.apple.com,
   under Sign-In and Security. Then run this in a terminal. It asks for your Apple ID
   and that password, reads your team ID from the certificate, and has Apple's
   `notarytool` save them in your keychain.

   ```sh
   scripts/notary-login.sh
   ```

If Apple refuses either step, look for a banner at developer.apple.com/account
first. When Apple updates its licence agreement, the account holder has to accept it
before certificates can be created or anything notarized.

Then:

```sh
scripts/bundle.sh --release               # for Macs with Apple silicon
scripts/bundle.sh --release --universal   # for Intel Macs as well
```

The script checks for the certificate before it builds anything. It then signs the
app with the hardened runtime, sends it to Apple and attaches the ticket that comes
back, builds and signs the disk image, sends that too, and finishes by asking
Gatekeeper what it would say on someone else's Mac. Each of the two submissions
usually takes a few minutes. Notarizing uploads the app to Apple; nothing from your
board is in it.

`--universal` needs the Intel build target once: `rustup target add
x86_64-apple-darwin`. With several Developer ID certificates in the keychain, name
the one to use in `JOBSEARCH_SIGN_IDENTITY`. A different credentials name goes in
`JOBSEARCH_NOTARY_PROFILE`.

Whoever you give it to gets their own empty board and their own settings. Yours stay
in your Library folder and are not part of the app.

Downloads are published at https://github.com/mattdobbs/jobsearch-releases, a public
repository that holds only the disk images and a page describing the app, so that
this repository can stay private. To publish a new version, add a release there and
attach the `.dmg`.

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

The "When a job becomes a card" column describes the board as it starts out. With
**Only show jobs from employers that welcome disabled and neurodivergent
applicants** turned off in Settings, every job from these sources that matches your
settings becomes a card. A card from one of the employers above still carries
its label; the others carry none, and say so when opened.

### The same job twice

Employers repost a job under a new number, and list one job once for each location.
A posting is not added when a card with the same employer and the same title is
already in a column other than **New** and **Evaluating**: **Applied to**, **Pass**,
or a column you added. Once you have applied to a job or passed on it, it does not
come back as new.

While a card is still in New or Evaluating, another posting with its title is added
as usual, because you have not decided anything yet. Cards already on the board are
never removed by this rule. Restoring a card from Pass lets new postings of that job
through again. The last check's **Duplicate** count includes the postings kept off
this way.

A job board's copy of a posting is also left out when the same job is already on the
board, in any column.

### LinkedIn

LinkedIn is not read. Tested on 2026-11-16, its public job search ignored the remote
filter and returned on-site jobs, and the link to the employer's own listing is only
shown after signing in. Signing the app in to your LinkedIn account would put that
account at risk of being restricted, which is a bad trade while job hunting. Reading
employers' career sites directly gets the same postings at their source, with a
dependable remote label.

### Remote, hybrid and on-site

The board starts out remote-only. **Kinds of job to show** in Settings adds hybrid
jobs, on-site jobs, or both. Untick Remote to leave remote jobs out.

A posting counts as **remote** only if:

1. the source marks it remote in a structured field, or the board lists remote jobs
   only, or the location or title plainly says remote; **and**
2. nothing in the title or location says hybrid, on-site or in-office; **and**
3. the description does not say this job is hybrid or on-site (for example "this is
   a hybrid role", "three days per week in the office"); **and**
4. the employer's own workplace label, where it has one, says remote and nothing
   else. "Hybrid or 100% Remote", "Remote/Hybrid" and "Flexible" are not remote,
   whatever the title says.

When a listing is ambiguous it is not counted as remote. That means some genuinely
remote jobs will be missed. An employer can also label a job remote and still mean
something else by it, so read the posting before applying.

A posting that is not remote is one of:

- **Hybrid** — its title, location or the employer's label says hybrid (on its own,
  beside remote, or as "flexible"), or its description describes a split week:
  "hybrid schedule", "three days per week in the office".
- **On-site** — one of those says on-site, in-office or in-person, or the description
  says the job is not remote or that you must relocate.
- **Workplace not stated** — nothing says either way. These come with the on-site
  jobs, because a posting that never mentions remote work is usually an on-site job.
  The card says "Workplace not stated" so you can tell the two apart.

Once a board holds more than remote jobs, every card says which kind it is.

Hybrid and on-site jobs are tied to a place. **Places I can travel to**, which
appears in Settings when either is ticked, keeps them to locations that name one of
your entries: a city, a state, or however your area is written. Remote jobs are not
held to it. Left empty, any hybrid or on-site job within "Where I can work from" is
shown, which for a whole country is a great many.

What is read for the extra kinds depends on the source:

- Career sites on Greenhouse, Lever and Ashby send their whole list, so every job on
  them is considered.
- A Workday site can hold tens of thousands of postings, so it is asked for each
  kind you ticked. Where the site has its own workplace filter, that is used.
  Otherwise the site is searched for "remote" or "hybrid", and for on-site jobs the
  newest 400 postings are read. On a large employer's site an on-site job is
  therefore found while it is new, not if it was posted long ago.
- The six remote job boards list remote jobs only. Hire Autism is asked for all of
  its listings.

A check takes longer with more kinds ticked: about eight minutes with all three,
against two for remote only. It also reads many more full descriptions. At most 60
are read from one site in one check, with a pause between them, and a site that says
it is getting too many requests is left alone until the next check. The rest wait
their turn, shown as **Later** in the last check, so the first few checks after
ticking a new kind keep adding cards.

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
- **Kinds of job to show** — Remote, Hybrid, and On-site. Remote only to begin with.
  See "Remote, hybrid and on-site" above.
- **Places I can travel to** — shown once Hybrid or On-site is ticked. A hybrid or
  on-site job has to name one of these places.
- **Only show jobs from employers that welcome disabled and neurodivergent
  applicants** — on to begin with. Turn it off to see every job that matches the
  settings above, whoever the employer is.

If a check finds postings but your settings leave all of them out, the New column
says so and links to Settings.

Changes to roles, places, kinds of job and the inclusive-employers setting apply from
the next check, and jobs already on the board stay there. The two hide lists also apply to the
board straight away, as described next.

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
