# Live check with a test account

What the scripted tests cannot prove: the parts built from PyMax hints or guesses, checked
against the real server. Use a test account only, never a personal one.

## Run

```powershell
cd C:\dev\maxrs
cargo run -p maxrs -- --trace
```

The trace goes to `%LOCALAPPDATA%\maxrs\traces\trace-<time>.log`; the path is printed at start.
It holds one line per frame: direction, `cmd`, `seq`, opcode and the payload with personal data
removed (`max_proto::redact`): map keys, numbers, enum names and error texts stay; message
texts, names and other strings become `<str N>`; phone numbers, tokens and codes become
`<hidden>`; URLs keep only their host. HTTP results and failures are lines starting with `--`.
Read the file before passing it on.

The login is stored in the `default` profile (`%LOCALAPPDATA%\maxrs\profiles\default`, key in
Windows Credential Manager under `maxrs`). Logout is not built yet: to end the test session,
close it in the phone app's list of active sessions and delete the profile folder.

## Steps

| # | Do | Confirms | Open point |
|---|----|----------|------------|
| 1 | Log in by QR, scanning with the test account's phone app | `CREATE_QR` → `AUTH_CHECK` → `AUTH_BY_TRACK`, then `LOGIN` | — |
| 1b | Or log in by phone number and SMS code | `AUTH_REQUEST` / `AUTH` with a desktop identity | MAP question 6 |
| 2 | Wait for the chat list | `LOGIN` with sync markers 0; names in the list | sync markers |
| 3 | Open a chat with 40+ messages | `CHAT_HISTORY` params (`from`, `backward`, the rest 0) | PyMax hint |
| 4 | Scroll to the top | the older page arrives without duplicates | `from` = oldest time |
| 5 | Watch the phone: the chat shows read | `CHAT_MARK` `READ_MESSAGE`, `mark` = now | PyMax hint |
| 6 | Send a text | `MSG_SEND`, the message reaches the phone | — |
| 7 | Send a text from the phone | `NOTIF_MESSAGE` (128) shows in the open chat | — |
| 8 | Open a chat with a photo | the photo loads from `baseUrl` alone | `--  image` lines |
| 9 | Send a photo from disk | `PHOTO_UPLOAD` URL, multipart field `file`, token, `MSG_SEND` | PyMax hint |
| 10 | Send a non-image file | `FILE_UPLOAD`, `Content-Range`, `NOTIF_ATTACH` (136) | PyMax hint |
| 11 | Download a file from a message | `FILE_DOWNLOAD` URL, saved in Downloads | — |
| 12 | Turn the network off and on | reconnect, `LOGIN` with the stored token | — |
| 13 | Restart the client | no new login; chat list from the stored token | — |

For each step note: worked / server error (the `error` and `localizedMessage` in the trace) /
nothing happened. Any server `cmd=Error` (3) line answers which guess was wrong.

## Results

Run 1 (phone login): `AUTH_REQUEST` accepted with the desktop identity; `requestMaxDuration` and
`altActionDuration` are in ms. `AUTH` CHECK_CODE refused with
`error.profile.active.session.no2fa` (MAP question 6).

Run 2 (QR login):

| # | Result |
|---|--------|
| 1 | works: `CREATE_QR` → `AUTH_CHECK` ×3 → `AUTH_BY_TRACK` → `LOGIN` |
| 2 | works; `LOGIN` with zero sync markers has no `contacts`, so dialog peers were bare ids (now fetched with `CONTACT_INFO`) |
| 3 | works: `CHAT_HISTORY` with `from` = now, `backward` 40, the rest 0 is accepted |
| 5 | accepted: `CHAT_MARK` answers `{mark, unread: 0}`; `mark` in the answer is the server's own |
| 8 | fails: every `i.oneme.ru` image request failed before an HTTP status ("error sending request"); the cause chain is now logged |

Channel posts have no `sender`; they were shown as ours (fixed). Steps 4, 6, 7, 9–13 not done yet.

Run 3 (stored token): login without QR (13 works); `CONTACT_INFO` with `contactIds` only is
accepted and dialog names show; channel posts show as incoming. Images failed with
`invalid peer certificate: UnknownIssuer`: `i.oneme.ru` is not under the Russian root. ADR-011
amended: MAX hosts accept the Russian root or a Mozilla root.

Run 4: photos load (58 `i.oneme.ru` images, no failures) with the amended ADR-011; step 8 works.
Same run, continued: a text to Saved messages (`chatId` 0) is stored (6 works); a photo goes
`PHOTO_UPLOAD` `{count: 1}` → upload to `iu.oneme.ru` → `MSG_SEND` with `photoToken` and comes
back with `baseUrl` (9 works). The client also sent `CHAT_MARK` for its own messages; it no
longer does. No pushes arrived during the run.
Still to do: 4, 7, 10, 11, 12.
