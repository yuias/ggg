# Scripts

Scripts are JavaScript files that hook into the download lifecycle. They can
change a request before it is sent (URL, headers, user agent), supply login
credentials, rename or move a finished file, and react to errors and progress.
Ready-made scripts are in `examples/scripts/`; copy one into
`<config_dir>/scripts/` to use it.

## Quick start

1. Check `settings.toml` (these are the defaults):

   ```toml
   [scripts]
   enabled = true
   directory = "./scripts"  # relative to the config directory; default is <config_dir>/scripts
   timeout = 30             # seconds
   ```

2. Create `<config_dir>/scripts/twitter_referer.js`:

   ```javascript
   ggg.on('beforeRequest', function (e) {
       e.headers['Referer'] = 'https://twitter.com/';
       ggg.log('Added Referer for ' + e.url);
       return true;
   }, 'twimg.com');
   ```

3. Start ggg. Scripts are loaded at startup and run for every download. The
   last argument, `'twimg.com'`, limits the handler to matching URLs.

## Hooks

Register a handler with `ggg.on(eventName, callback, [filter])`. The callback
receives one event object. Change the fields marked "writable" and ggg uses
the new values. Changes to other fields are ignored.

| Hook              | Runs                                      | Waits for the handler |
| ----------------- | ----------------------------------------- | --------------------- |
| `beforeRequest`   | Before the HTTP request is sent           | yes                   |
| `headersReceived` | After the server answers, before the body | yes                   |
| `authRequired`    | When the server answers 401 or 407        | yes                   |
| `completed`       | After the file has been downloaded        | yes                   |
| `error`           | After a download failed for good          | no                    |
| `progress`        | While a download runs                     | no                    |

Fields shown as "may be null" are `null` when the value is unknown.

### beforeRequest

```javascript
{
    url,        // writable
    headers,    // writable, object of string values
    userAgent,  // writable, may be null
    downloadId  // read-only
}
```

```javascript
ggg.on('beforeRequest', function (e) {
    e.url = e.url.replace('old-domain.com', 'new-domain.com');
    e.headers['Authorization'] = 'Bearer YOUR_TOKEN';
    return true;
}, 'old-domain.com');
```

### headersReceived

Read-only; the event cannot be changed.

```javascript
{
    url, status, headers,
    contentLength,  // may be null
    etag,           // may be null
    lastModified,   // may be null
    contentType     // may be null
}
```

```javascript
ggg.on('headersReceived', function (e) {
    ggg.log(e.url + ': ' + e.status + ' ' + e.contentType);
});
```

### authRequired

```javascript
{
    url,
    realm,     // may be null
    username,  // writable, always null on entry
    password   // writable, always null on entry
}
```

ggg never passes stored credentials to scripts. If a handler sets both
`username` and `password`, ggg sends them as an HTTP Basic `Authorization`
header and repeats the request. If the server rejects them, or if no handler
sets both fields, the download fails.

```javascript
ggg.on('authRequired', function (e) {
    e.username = 'user';
    e.password = 'secret';
    return true;
}, 'files.example.com');
```

### completed

```javascript
{
    url, filename,
    savePath,      // directory the file was saved to
    size,          // bytes
    duration,      // seconds, may be null
    newFilename,   // writable, null on entry: set it to rename the file
    moveToPath     // writable, null on entry: set it to move the file
}
```

- `newFilename` is reduced to a bare file name (path separators and other
  unsafe characters are removed). If the name already exists in the folder, ggg
  makes it unique.
- `moveToPath` is a directory that must already exist; ggg does not create it.
  Use an absolute path. A path containing `..` is rejected.
- The rename happens first, then the move. A failed rename or move is logged
  and the file stays where it is.

```javascript
ggg.on('completed', function (e) {
    if (/\.(jpg|png|gif)$/i.test(e.filename)) {
        e.moveToPath = '/path/to/images';
    }
    return true;
});
```

### error

Read-only. Runs once per download, after all retries are used up.

```javascript
{
    url,
    filename,    // may be null
    error,       // message
    retryCount,
    statusCode   // may be null
}
```

```javascript
ggg.on('error', function (e) {
    ggg.log('Failed: ' + e.url + ' (' + e.error + ')');
});
```

### progress

Read-only. Runs at most about twice per second for each download.

```javascript
{
    url, filename,
    downloaded,   // bytes so far
    total,        // bytes, may be null
    speed,        // average bytes per second since the download started, may be null
    percentage    // currently always null: compute it from downloaded and total
}
```

```javascript
ggg.on('progress', function (e) {
    if (e.total) {
        ggg.log(e.filename + ': ' + (e.downloaded / e.total * 100).toFixed(0) + '%');
    }
});
```

## API

### ggg.on(eventName, callback, [filter])

- `eventName`: one of `beforeRequest`, `headersReceived`, `authRequired`,
  `completed`, `error`, `progress`. The aliases `onBeforeRequest`,
  `onHeadersReceived`, `onAuthRequired`, `complete`, `onCompleted`,
  `errorOccurred`, `onErrorOccurred` and `onProgress` are also accepted.
- `callback`: a function that receives the event object.
- `filter` (optional string): the handler only runs for URLs that match.

`ggg.on` returns `true`, which is not useful. It throws if `callback` is not a
function. An unknown `eventName` is detected when the script finishes loading.
In both cases the script registers no handlers.

**Filters.** The filter is matched against the event's `url` and is not
anchored.

- A filter containing `*`, `^` or `$` is a regular expression (Rust `regex`
  syntax). Write `'\\.jpg$'`, not `'*.jpg'`: `*` is not a glob wildcard.
- Any other filter is a plain substring match; `.` and other special
  characters are taken literally.
- An invalid regular expression makes the whole script fail to load.
- Pass a string. Other values (such as a `RegExp` object) are ignored and the
  handler runs for every URL.

### ggg.log(message) and console

`ggg.log` and the `console` methods write to the application log (see
[Troubleshooting](#troubleshooting)) as `[Script] <message>`. `console.log`
and `console.info` add nothing to the message; `console.warn`, `console.error`
and `console.debug` prepend `[WARN]`, `[ERROR]` and `[DEBUG]`. All entries are
logged at INFO level.

### Return values

A handler may return nothing. Only an explicit `false` has an effect: later
handlers for the same event are skipped. The download itself continues, and
changes the handler made to the event are still applied.

A handler that throws, or runs longer than `timeout`, is logged and skipped,
and its changes to the event are discarded. The remaining handlers still run.

## Loading and reloading

### Loading order

ggg loads the `.js` files directly inside the scripts directory, sorted by
file name. Subdirectories and symbolic links are skipped, and a file larger
than 1 MiB is rejected. Each file is run once at startup, and its `ggg.on`
calls register the handlers. If the directory does not exist, ggg creates it.

Because the order follows the file names, use numeric prefixes to control it:
`01_headers.js` runs before `02_rename.js`.

For each event, handlers run in this order:

1. Scripts in file-name order.
2. Within a script, handlers in the order of their `ggg.on` calls.
3. Handlers stop at the first one that returns `false`.

A script that throws while loading, or that uses an unknown event name or an
invalid filter, registers no handlers. The other scripts still load, and the
error is logged.

### Reloading

To pick up edits to scripts without restarting:

- **Daemon:** start ggg with `ggg --headless`, then run `ggg script reload`.
  Without a running daemon the command only prints a notice.
- **TUI:** open Settings (`x`), switch to the Scripts tab and press the
  reload key (`R` by default, see
  [Keybindings](Keybindings.md#default-bindings)).

A reload runs every script again. It does not re-read `enabled`, `directory`
or `timeout` from the settings; those take effect at the next start.

## Configuration

See [examples/settings.toml.example](../examples/settings.toml.example) for
the full syntax.

| Setting               | Default                | Notes                                                             |
| --------------------- | ---------------------- | ----------------------------------------------------------------- |
| `[scripts] enabled`   | `true`                 | `false` turns scripting off. Takes effect at the next start.      |
| `[scripts] directory` | `<config_dir>/scripts` | A relative path is resolved against the config directory.         |
| `[scripts] timeout`   | `30`                   | Seconds. Applies to each handler call and to loading each script. |

### Disabling single scripts

List a file name under `[scripts.script_files]` to switch that script off:

```toml
[scripts.script_files]
"pixiv.js" = false
```

A script that is not listed is enabled. A folder's settings file
(`<config_dir>/<folder_name>/settings.toml`) can override this for that folder
with its own `[script_files]` table. These maps are read each time a hook runs,
so changes do not need a reload.

The same switches are available in the Settings > Scripts tab (select a script
and press `Enter` or `Space`), in the folder settings editor, and with
`ggg script list`, `ggg script enable <file>` and `ggg script disable <file>`.

A folder file may also contain `scripts_enabled`, but hooks do not check it;
use `script_files` to switch scripts off for a folder.

## Example scripts

### Custom user agent per domain

```javascript
const agents = {
    'example.com': 'MyCustomBot/1.0',
    'test.org': 'TestClient/2.0'
};

ggg.on('beforeRequest', function (e) {
    for (const domain in agents) {
        if (e.url.includes(domain)) {
            e.userAgent = agents[domain];
            break;
        }
    }
    return true;
});
```

### Scripts in `examples/scripts/`

- `pixiv.js`: sets the Referer and User-Agent for Pixiv image downloads.
- `filename_cleanup.js`: after a download, decodes URL-encoded names, collapses
  repeated spaces and removes a trailing "(...)" part.
- `error_notifier.js`: logs the URL, error and retry count of failed downloads.
- `progress_logger.js`: logs progress and speed at 25, 50, 75 and 100 percent.
  It only logs when an update lands exactly on one of those values, so treat it
  as a starting point.

## Troubleshooting

**Where the log is.** ggg writes JSON lines to
`<config_dir>/.logs/app.jsonl.<date>`, one file per day, at INFO level
(`-v` or `--verbose` lowers it to TRACE). Show it with `ggg logs`, or follow
it with `ggg logs -f`. Script messages start with `[Script] `.

**A script does nothing.**

- `[scripts] enabled` must be `true`, and ggg must have been restarted after
  you changed it.
- The file must end in `.js` and sit directly in the scripts directory.
- Run `ggg script list` to see which scripts ggg finds and whether they are
  enabled.
- Look in the log for `Failed to load script`. The cause is usually a syntax
  error, an unknown event name or an invalid filter.
- Check the filter. It is matched against the event's URL, not the file name.
- An earlier handler may have returned `false`.

**A handler runs but has no effect.** Look in the log for `Script execution
error` (the handler threw) or `Script timeout` (it ran longer than `timeout`).
In both cases its changes were discarded.

**Edits are not picked up.** Reload the scripts, see [Reloading](#reloading).

**Try a script without a download.** `ggg script test <file>.js --event
beforeRequest --url <url>` runs the script's `beforeRequest` handlers against a
test URL and prints the resulting URL and headers. Other events are not
supported by this command.

**Keep handlers short.** One script thread runs every handler one at a time,
and the hooks marked "yes" in the table above hold up the download until the
handler returns. Use a filter so a handler only runs for the URLs it needs.

## Limits and security

- Scripts run in an embedded V8 engine (`deno_core`). ggg adds no
  file-system or network API; the only additions are `ggg.*` and `console.*`.
- A handler call, and the loading of a script, is stopped after `timeout`
  seconds. The V8 heap is capped at 256 MiB.
- Scripts can still change request URLs, headers and user agents, provide
  login credentials, and rename and move finished files. They also see the
  request headers of every download. Only install scripts you trust.
