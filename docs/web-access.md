# Web Access

Web Access is an optional browser interface and API for the Degauss frontend.
It is Off by default. It runs inside Degauss, not in a separate service, game
core or MiSTer Main. No web process is left running while a game is playing.

## Connect

1. On MiSTer, open **Options → Web Access → Web Password** and set a password
   of 8 to 128 characters. This is separate from a kiosk PIN.
2. Choose **HTTPS** under **Web Server**. The default port is **8443**; **Web
   Port** accepts ports 1024 to 65535.
3. Open **Open Address / QR Code**. Enter the complete address it shows, including
   `https://` and the port, in a browser on the same local network. The QR code
   opens that same address. A CRT shows integer-sized black/white QR modules;
   the text address remains available if the phone cannot read the display.
4. For HTTPS, the browser initially warns about a self-signed certificate.
   Check that the address is the one displayed on MiSTer before trusting this
   device's certificate. This is a private-device certificate, not a public
   website certificate. Trust controls differ between browsers. If the browser
   blocks self-signed certificates entirely, use its certificate-management
   instructions or explicitly choose HTTP on a trusted LAN.
5. Sign in. **Remember this browser** keeps the session for 30 days. Without it,
   the session lasts up to 12 hours and is not restored after Degauss restarts.
   The browser stores a session cookie, not the password.

**HTTP is also available**, with the same password and selected port, but traffic
and credentials are not encrypted. The setting selects one protocol, not both
listeners simultaneously. Degauss never silently falls back from HTTPS to HTTP.
Use the exact address shown after changing the protocol or port.

HTTPS identity is saved on the card. Normal restarts keep it. If the device gets
a new network address, **Renew HTTPS Certificate** generates a certificate for
the new address; trust that replacement in the browser. A saved DHCP reservation
avoids unnecessary address changes. A certificate error is shown on MiSTer,
not hidden by changing protocols.

Keep Web Access on a trusted local network. Do not forward its port on a router
or publish it to the internet. No cloud account, CDN, browser extension, Node.js
runtime or MiSTer Main change is required. **Off** closes the listener. Changing
the password revokes previous sessions. **Sign out** revokes that browser session.

## Browse and organise

**View as** applies throughout browsing pages in this browser session. It starts
at Grid after reloading the page, and never writes native layout preferences:

| Web view | Presentation |
| --- | --- |
| Grid | Artwork tiles with names and actions. |
| List | Compact rows with small artwork. |
| Details | Select a row to read its artwork and full information beside the list. |
| Gallery | One large image with full information and Previous/Next navigation. |
| Carousel | A dominant selected image with neighbouring entries; open Details when needed. |

All five retain the current search, filters and ordering. Previous/Next in the
selected-entry views continues across result pages. Page and Letter jumps remain
available below the results. Game descriptions are read only when needed for the
selected entry, not for every off-screen game. **Change MiSTer layout** and the
Layouts Designer are separate controls that intentionally change native layouts.

**Options → Web Access → CRT Image Effect** adds the website-style scanlines,
subtle RGB pattern and shaded edges to browser artwork and logos. It is Off by
default. Changes appear in connected browsers without signing out or reloading
images. Text, QR codes, original image files and MiSTer's display are unchanged.

- The Degauss logo returns to Home. Breadcrumbs above the page title link to
  parent categories, systems and folders. A category with only one system opens
  its games directly, including Arcade and Favourites on a standard setup.
- Open Favourites from Home, then use **Manage favourites folders** for folder
  operations. These controls are not shown in unrelated systems.
- Browse the existing library, folders and ZIP contents, with artwork and game
  information from the same selected data source as the native frontend.
- Use the Page and Letter selectors below paged lists to jump directly within
  the current results. They also work in Explore, saved collections and Core
  Updates, without clearing filters or changing the ordering.
- Opening a game immediately shows its selected artwork, available information
  and controls. A visible reading status remains until the full description is
  loaded. Further games in that metadata source reuse its parsed descriptions;
  changing the source file refreshes them automatically.
- Search and filter in Games Explorer, group by metadata and save collections.
- **Random**, beside a folder or collection title, offers **Random Game** and
  **Random Favourite**. Both keep the current search and filters and open the
  chosen game's details without launching it. Picks cover the whole matching
  list, not just the displayed page. An unfiltered folder with no eligible game
  follows native behaviour by picking a child folder and looking inside it.
- Edit Home and personal folders, drag entries to reorder them, or use the
  equivalent arrow buttons. Add, rename, move, hide and remove references.
  Personal folders retain the native three-level nesting limit.
- Add shared MiSTer favourites, choose another filename when a name is occupied,
  manage favourite folders, and browse Last Played.
- Edit native layout definitions, preview panel proportions, and assign them to
  Home, categories, systems and folders. Exactly one list and the native divider
  limits still apply. A folder can follow the global view independently.
- Edit themes and choose fonts, colours and custom system/Home images.
- Select Gamelist or Artwork Pack sources and pack types, choose pack locations
  and manual matches, run Libretro or ScreenScraper jobs and inspect progress.
  A manual single-game scrape replaces its image; batch jobs retain their native
  Missing Only setting. Artwork Pack precedence is never silently changed.
- Upload PNG/JPEG artwork or custom logos, up to 16 MiB. Uploads are decoded and
  checked before installation. Game images update the native Gamelist through
  the existing backup/save path; they do not modify a downloaded Artwork Pack.
- Browse Core Updates and What's New, refresh configured sources, inspect
  matching game titles/artwork, choose installed cores and set compatible core
  preferences for systems or games.
- Change software settings and folder preferences, manage saved controller
  preferences, start Attract Mode, rebuild indexes and launch installed scripts.

The browser has its own navigation. Browsing on a phone does not move the
selection on the MiSTer display. **Remote Control** deliberately sends the native
navigation/actions buttons. Physical controller capture/testing and visual
screen calibration still use the MiSTer display and hardware.

Organisation edits do not move, rename, upload or delete ROMs. There is no general
file manager, arbitrary command endpoint or gameplay streaming. Kiosk restrictions
also apply to browser actions; a web password does not bypass the kiosk PIN.

## Launch, return and concurrent changes

After Degauss acknowledges a game or script launch, the page shows that Web Access
is paused and reconnects when Degauss returns. An unexpected connection loss has a
different message. A newly opened browser cannot connect during gameplay. An
interactive script uses MiSTer's display and terminal, not a browser terminal.

Commands are never replayed after a disconnection. If a response was lost, inspect
the current state before retrying an edit or launch. Changing Web Access settings
can disconnect the browser; use the new address displayed on MiSTer.

Edits include the revision that was displayed when the editor opened. If another
browser or the native frontend changes those settings, an old editor receives a
conflict instead of overwriting the newer settings. Refresh and reapply the edit.
Long-running reads and writes show progress. Cancellation is available while the
underlying native operation can stop safely; an atomic installation already in
progress must finish.
Page changes show **Loading**, and background library reads show **Reading**
with their current progress above the content.

## API v1

The embedded web client uses this API; native clients can use it too. The matching
[OpenAPI specification](../web/openapi.json) is served at
`/api/v1/openapi.json` after login. All API requests require a current session
except login. The frontend's version is returned by the `state` command.

1. `POST /api/v1/login` with JSON `{"password":"…","remember":true}`.
   Retain the returned cookie and `csrf` token.
2. Send `X-Degauss-CSRF` on every subsequent POST, including read commands.
   `GET /api/v1/session` returns the current token after reconnecting.
3. `POST /api/v1/command` with a tagged JSON object, for example
   `{"action":"systems"}` or
   `{"action":"browse","system":"NES","offset":0,"limit":60}`.
4. A response containing `job` is asynchronous. Send `{"action":"job","id":…}`
   until its state is `complete`, `failed` or `cancelled`. Completion has a `result`;
   failure has an `error` with `code` and `message`. Polling does not repeat work.
5. `GET /api/v1/events` supplies server-sent `state`, `job` and `resync` events.
   Re-read state after `resync`. There is no write channel on this stream.
6. `POST /api/v1/logout` revokes the current session.

Identifiers and media URLs come from API responses. Do not construct filesystem
paths. Home, browse and scripts responses include a `breadcrumbs` array of names
and registered `folder` identifiers; use those identifiers to open ancestors.
Media URLs require the same session. Keep the entire revision string from
the result used to open an editor and submit it unchanged with the edit. A
successful later read must not replace the revision of an already open editor.

The API accepts at most 200 rows per library/Explore page. Ordinary JSON bodies
are limited to 1 MiB. Use the binary upload endpoint for images. There are bounded
request, event-stream and worker queues. Up to eight requests run concurrently;
normal image bursts wait up to five seconds for a slot before a `busy` response.
A `busy` error means retry only after
checking the current job/state. HTTP 409 means an edit conflict. Other failures
carry an actionable error. A `durability` error explicitly means a settings file
was installed but the filesystem could not confirm its directory flush; re-read
state before changing it again.

### Command catalogue

`?` means optional or null. All settings edits below that take `revision` use the
current revision from their editor's read response. Commands reject unknown fields.

| Area | Commands and additional fields |
| --- | --- |
| State | `state`, `systems`, `options` |
| Connection | `web-settings`; `web-settings-save` (`revision`, `config`, `password?`); `web-certificate-renew` |
| Library | `browse` (`system`, `folder?`, `offset?`, `limit?`, `search?`, `filters?`); `game` (`id`); `history` |
| Explore | `explore` (`query`, `offset?`, `limit?`); `random` (`query?`, `folder?`, `favorites_only?`) |
| Home | `home`, `home-artwork` (`folder?`); `home-edit` (`revision`, `edit`) |
| Collections | `collections`; `collection-save` (`revision`, `id?`, `name`, `query`); `collection-delete` (`revision`, `id`) |
| Views | `views` (`target?`); `view-save` (`revision`, `id?`, `definition`); `view-delete` (`revision`, `id`); `view-assign` (`revision`, `target?`, `view`) |
| Folder settings | `folder-options` (`target`); `folder-options-save` (`revision`, `target`, `preferences`); `hide` (`revision`, `target`, `hidden`) |
| Themes | `themes`; `theme-select` (`revision`, `name?`); `theme-save` (`revision`, `name`, `text`, `replace?`); `theme-delete` (`revision`, `name`) |
| Scrapers | `scraper-settings`; `scraper-settings-save` (`revision`, `values`, `password?`, `accept_plaintext_storage?`); `scraper-search` (`game`, `term`); `scraper-preview` (`match_id`); `scrape` (`target?`, `match_id?`) |
| Data source | `game-data` (`system`); `game-data-locations` (`system`, `picture_type`, `directory?`); `game-data-save` (`revision`, `system`, `source`, `location?`, `picture_type?`) |
| Pack matching | `pack-matches` (`game`, `search?`); `pack-match-save` (`revision`, `game`, `choice?`) |
| Favourites | `favorite-folders`; `favorite-add` (`game`, `folder?`, `name?`); `favorite-remove` (`game`); `favorite-folder-create` (`name`); `favorite-folder-rename` (`folder`, `name`); `favorite-folder-delete` (`folder`) |
| Images | `images`; `image-choose` (`revision`, `target`, `image?`) |
| Core choice | `cores` (`category?`); `core-choices` (`system`, `game?`); `core-select` (`revision`, `system`, `game?`, `family?`, `version?`) |
| Core Updates | `core-updates` (`begin?`, `refresh?`); `core-update` (`key`); `core-updates-seen` (`snapshot`) |
| Controllers | `controllers`; `controller-save` (`revision`, `id`, `profile`) |
| Settings | `option-adjust` (`revision`, `option`, `delta`); `reset` (`revision`, `kind`); `ini-profiles`; `ini-switch` (`slot`) |
| Control | `scripts` (`folder?`); `launch` (`id`); `remote` (`button`); `unlock` (`pin`); `attract` |
| Work | `rebuild` (`system?`); `job` (`id`); `cancel` (`id`) |

### Native data contracts

- **Live presentation:** `state` returns `theme`, `font`, `text_size` and
  `crt_images`. The latter is the native CRT Image Effect preference for browser
  images only. State events update connected clients without changing the server
  protocol, port or session. Independent browser view selection is client state,
  not a native layout assignment.
- **Pagination:** Browse and Explore return `total`, `offset`, `limit` and
  `letters`, an array of `{letter, offset}` entries for the whole filtered list.
  A letter's offset identifies its first entry in the current ordering. Send it
  as the next request's `offset`, retaining the query. Non-alphanumeric initials
  use `#`. Core Updates pagination is client-side over its returned snapshot.
  Previous, the displayed range and Next occupy the first pagination row; Page
  and Letter jumps occupy the row below.
- **Query:** `title`, optional category/system, optional decade criterion and a
  `fields` map keyed by Genre, Year, Players, Language, Developer or Publisher.
  Use criteria returned by the facet response. A known criterion is
  `{"Known":{"label":"…","key":"…"}}`; unknown is `"Unknown"`.
- **Home edit:** tagged `operation` is `add`, `folder`, `rename`, `remove`, `move`,
  `reorder` or `visibility`. Add uses a registered `target`, `name` and optional
  `parent`; folder creates a named folder; move uses `id` and `parent`; reorder
  supplies the complete ordered `keys` for a `folder`; visibility uses `key` and
  `visible`. Removing a personal folder removes references beneath it, not games.
- **Views:** `definition` uses `name`, `template`, `panels`, `columns` and `rows`.
  Templates are `columns`, `split-right`, `split-left`, `stacked`. Panel types are
  `list`, `artwork`, `short-information`, `full-information`. Splits are 20–80
  percent. The UI offers the valid native choices. A null assignment target is
  global; `home`, `explore`, `history`, `cores`, `cores:<installed category>` or a
  registered folder target selects a specific place. An empty `view` removes
  that place's override. Fixed native screens have no layout selector.
- **Folder preferences:** independently optional `screen_rotation`, `folders_last`
  and `game_name_display` use native settings values. Null means follow global.
- **Themes:** `text` is native theme TOML. The editor may replace/delete only
  editor-created themes. Other themes can be copied and saved under a new name.
  Replies include `notices` when the selected theme no longer loads. The saved
  name is retained and the standard palette is used, matching native startup.
- **Controller profiles:** use the complete `custom` and `bindings` object returned
  by `controllers`. Bindings map native role names to arrays of `{code,label}`.
  Only already captured controllers can be edited; hardware capture stays native.
- **Scraper settings:** edit the typed `values` returned by `scraper-settings`.
  Credentials are never returned. A new ScreenScraper password requires explicit
  acknowledgement of the existing plaintext-on-card credential storage contract.
- **Pack source:** `automatic`, `gamelist` or `artwork-pack`; pack type is
  `box-art`, `screenshot` or `title-screen`. Null type follows the global default.
  `location` and match `choice` are handles returned by their corresponding read.
- **Core Updates:** `begin:true` starts a visit; `refresh:true` explicitly requests
  network refresh. Acknowledge the returned snapshot only after presenting it.
  Opening the browser alone does not mark an unseen change as seen.
- **Options:** use IDs returned by `options`; adjustable options accept delta
  `-1` or `1`. Dedicated editors use their corresponding commands. Reset kind is
  `view-selections` or `hidden`. Game core versions remain per-system; a game
  override selects a compatible family.
- **Upload:** `POST /api/v1/upload/{target}` with raw PNG/JPEG bytes, session,
  CSRF and `X-Degauss-Revision`. A game target writes its Gamelist image. A Home
  entry/category/system target imports a logo and chooses it after validation.

Web state is stored beside Degauss settings in `web-access/`, including the salted
password hash, remembered-session records and HTTPS identity/private key. Keep
that directory private and include it in card backups. Do not publish it with
diagnostics. Existing game data, native settings and launch paths remain in their
existing formats.
