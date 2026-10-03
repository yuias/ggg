# Script Hook System User Guide

The script hook system allows you to customize download behavior using JavaScript scripts.

## Quick Start

### 1. Enable Scripts in Configuration

Edit your `settings.toml`:

```toml
[scripts]
enabled = true
directory = "./scripts"  # Relative to config directory
timeout = 30
```

### 2. Create a Script

Create a `.js` file in the `scripts/` directory:

**Example: `scripts/twitter_referer.js`**
```javascript
// Add referer header for Twitter images
ggg.on('beforeRequest', function(e) {
    if (e.url.includes('twimg.com')) {
        e.headers['Referer'] = 'https://twitter.com/';
        ggg.log('Added Twitter referer for: ' + e.url);
    }
    return true;
});
```

### 3. Run Your Download Manager

Scripts are automatically loaded on startup and execute for every download.

## Available Hooks

All hooks are now fully implemented! ✅

### beforeRequest

**When:** Before HTTP request is made
**Can Modify:** URL, headers, user-agent
**Example Use Cases:**
- Add custom headers (Referer, Authorization)
- Modify URLs (add parameters, change domains)
- Set custom user-agents per site

**Event Object:**
```javascript
{
    url: string,           // Download URL (modifiable)
    headers: object,       // HTTP headers (modifiable)
    userAgent: string,     // User-Agent string (modifiable)
    downloadId: string     // Unique download ID (read-only)
}
```

**Example:**
```javascript
ggg.on('beforeRequest', function(e) {
    // Add authentication header
    if (e.url.includes('private-site.com')) {
        e.headers['Authorization'] = 'Bearer YOUR_TOKEN';
    }

    // Modify URL
    if (e.url.includes('old-domain.com')) {
        e.url = e.url.replace('old-domain.com', 'new-domain.com');
    }

    return true; // Continue to next handler
});
```

### headersReceived

**When:** After receiving server response headers (before download starts)
**Can Modify:** None (read-only inspection)
**Example Use Cases:**
- Log server information
- Validate content-type
- Check file size before downloading

**Event Object:**
```javascript
{
    url: string,              // Original request URL
    status: number,           // HTTP status code
    headers: object,          // Response headers
    contentLength: number,    // File size in bytes (if known)
    etag: string,            // ETag header (if present)
    lastModified: string,    // Last-Modified header (if present)
    contentType: string      // Content-Type header (if present)
}
```

**Example:**
```javascript
ggg.on('headersReceived', function(e) {
    ggg.log('Downloading: ' + e.url);
    ggg.log('Size: ' + (e.contentLength / 1024 / 1024).toFixed(2) + ' MB');
    ggg.log('Type: ' + e.contentType);
    return true;
});
```

### completed

**When:** After download completes successfully
**Can Modify:** Filename (rename), save path (move)
**Example Use Cases:**
- Rename files based on patterns
- Move files to different folders
- Clean up filenames
- Organize downloads by type

**Event Object:**
```javascript
{
    url: string,              // Original download URL
    filename: string,         // Current filename
    savePath: string,         // Current directory path
    size: number,            // File size in bytes
    duration: number,        // Download duration in seconds
    newFilename: string,     // Set to rename file (modifiable)
    moveToPath: string       // Set to move file (modifiable)
}
```

**Example:**
```javascript
ggg.on('completed', function(e) {
    // Clean up filename
    let cleaned = e.filename.replace(/[<>:"/\\|?*]/g, '_');
    if (cleaned !== e.filename) {
        e.newFilename = cleaned;
        ggg.log('Renamed: ' + e.filename + ' -> ' + cleaned);
    }

    // Move images to Images folder
    if (e.filename.match(/\.(jpg|png|gif)$/i)) {
        e.moveToPath = './downloads/images';
        ggg.log('Moved image to images folder');
    }

    return true;
});
```

### error

**When:** When download fails
**Can Modify:** None (fire-and-forget notification)
**Example Use Cases:**
- Log errors
- Send notifications
- Track failure patterns

**Event Object:**
```javascript
{
    url: string,           // Download URL
    filename: string,      // Filename (if known)
    error: string,         // Error message
    retryCount: number,    // Number of retries attempted
    statusCode: number     // HTTP status code (if applicable)
}
```

**Example:**
```javascript
ggg.on('error', function(e) {
    ggg.log('ERROR: Failed to download ' + e.url);
    ggg.log('Reason: ' + e.error);
    if (e.statusCode) {
        ggg.log('HTTP Status: ' + e.statusCode);
    }
    return true;
});
```

### progress

**When:** Periodically during download (throttled to ~500ms intervals)
**Can Modify:** None (fire-and-forget notification)
**Example Use Cases:**
- Log progress milestones
- Track download statistics
- Custom progress notifications

**Event Object:**
```javascript
{
    url: string,           // Download URL
    filename: string,      // Filename
    downloaded: number,    // Bytes downloaded so far
    total: number,         // Total bytes (if known)
    speed: number,         // Download speed (bytes/sec)
    percentage: number     // Download percentage (0-100, if total known)
}
```

**Example:**
```javascript
ggg.on('progress', function(e) {
    if (e.percentage && e.percentage % 25 === 0) {
        ggg.log(e.filename + ': ' + e.percentage + '% complete');
        let speedMB = (e.speed / 1024 / 1024).toFixed(2);
        ggg.log('Speed: ' + speedMB + ' MB/s');
    }
    return true;
});
```

## API Reference

### ggg.on(eventName, callback, [filter])

Register a hook handler.

**Parameters:**
- `eventName` (string): Event to listen for
  - `'beforeRequest'` - Before HTTP request
  - `'headersReceived'` - After receiving server headers
  - `'completed'` - After download completes
  - `'error'` - When download fails
  - `'progress'` - During download progress
- `callback` (function): Handler function receiving event object
- `filter` (string, optional): URL pattern to filter (substring or regex)

**Returns:** `true` (success)

**Example with Filter:**
```javascript
// Only execute for Twitter URLs
ggg.on('beforeRequest', function(e) {
    e.headers['Referer'] = 'https://twitter.com/';
    return true;
}, 'twimg.com');

// Regex filter
ggg.on('beforeRequest', function(e) {
    ggg.log('Downloading image: ' + e.url);
    return true;
}, '^https://.*\\.(jpg|png|gif)$');
```

### ggg.log(message)

Log a message to the application log.

**Parameters:**
- `message` (string): Message to log

**Example:**
```javascript
ggg.log('Script executed for: ' + e.url);
```

### Return Values

Handlers should return a boolean:
- `true` - Continue to next handler
- `false` - Stop propagation (no further handlers execute)

**Example:**
```javascript
ggg.on('beforeRequest', function(e) {
    if (e.url.includes('blocked-site.com')) {
        ggg.log('Blocking download from: ' + e.url);
        return false; // Stop here, don't download
    }
    return true; // Continue normally
});
```

## Script Loading

### Loading Order

Scripts are loaded **alphabetically** by filename:
1. `01_first.js`
2. `02_second.js`
3. `10_tenth.js`
4. `script.js`

**Tip:** Use numeric prefixes to control execution order.

### Reloading Scripts

Scripts are otherwise loaded once at startup. To pick up edits (new
scripts, changed handlers, enable/disable toggles) without restarting:

- **Daemon mode:** start ggg as a daemon (`ggg --headless`) and run
  `ggg script reload` against it from another terminal. Without a daemon
  running, `ggg script reload` prints a notice instead, since there is
  nothing to reload.
- **TUI:** open Settings (`x`) and switch to the Scripts tab, then press
  the reload-settings key (`R` by default; see
  [KeyBindings.md](KeyBindings.md#default-bindings)) to reload scripts
  instead of `settings.toml`.

### Handler Execution Order

For each event:
1. Scripts execute in alphabetical order
2. Within each script, handlers execute in registration order
3. If any handler returns `false`, remaining handlers are skipped

## Example Scripts

### Twitter Image Downloads

```javascript
// scripts/01_twitter_referer.js
ggg.on('beforeRequest', function(e) {
    if (e.url.includes('pbs.twimg.com') || e.url.includes('twimg.com')) {
        e.headers['Referer'] = 'https://twitter.com/';
        e.headers['User-Agent'] = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36';
        ggg.log('Added Twitter headers for: ' + e.url);
    }
    return true;
}, 'twimg.com');
```

### Pixiv Downloads

```javascript
// scripts/02_pixiv_headers.js
ggg.on('beforeRequest', function(e) {
    if (e.url.includes('pixiv.net') || e.url.includes('pximg.net')) {
        e.headers['Referer'] = 'https://www.pixiv.net/';
        e.userAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36';
        ggg.log('Added Pixiv headers');
    }
    return true;
}, 'pxi');
```

### Custom User-Agent Per Domain

```javascript
// scripts/03_custom_ua.js
const domains = {
    'example.com': 'MyCustomBot/1.0',
    'test.org': 'TestClient/2.0'
};

ggg.on('beforeRequest', function(e) {
    for (let domain in domains) {
        if (e.url.includes(domain)) {
            e.userAgent = domains[domain];
            ggg.log('Set user-agent for ' + domain);
            break;
        }
    }
    return true;
});
```

## Configuration

### Script Directory

Default: `<config_dir>/scripts` (resolved at runtime)

All `.js` files in this directory are loaded. Subdirectories are ignored.
Relative paths are resolved against the config directory.

```toml
[scripts]
directory = "./my-scripts"  # Relative to config directory
```

### Timeout

Maximum execution time for script handlers (default: 30 seconds).

```toml
[scripts]
timeout = 60  # Increase for slow scripts
```

### Enable/Disable

```toml
[scripts]
enabled = false  # Disable all scripts
```

## Debugging

### View Logs

Scripts use `ggg.log()` to output messages:

```bash
# Run with logging
RUST_LOG=info cargo run
```

Logs appear as:
```
[Script] Added Twitter referer for: https://pbs.twimg.com/media/...
```

### Test Scripts

1. Create a test script:
```javascript
ggg.on('beforeRequest', function(e) {
    ggg.log('=== BEFORE REQUEST ===');
    ggg.log('URL: ' + e.url);
    ggg.log('Headers: ' + JSON.stringify(e.headers));
    return true;
});
```

2. Start a download and watch logs

## Troubleshooting

### Script Not Loading

- Check `settings.toml` has `enabled = true`
- Verify script is in correct directory
- Check file extension is `.js`
- Look for syntax errors in application logs

### Script Not Executing

- Verify event name is correct (`'beforeRequest'`)
- Check URL filter matches your download
- Ensure script returns `true` to continue

### Syntax Errors

Scripts are JavaScript (ES5+). Common issues:
- Missing semicolons
- Incorrect function syntax
- Typos in API calls (use `ggg.log`, `console.log` also works but prefer `ggg.log`)

## Best Practices

1. **Use Filters** - Only execute for relevant URLs
   ```javascript
   ggg.on('beforeRequest', handler, 'specific-domain.com');
   ```

2. **Log Sparingly** - Avoid logging every request
   ```javascript
   if (e.url.includes('debug')) {
       ggg.log('Debug info: ' + e.url);
   }
   ```

3. **Handle Errors Gracefully** - Scripts shouldn't crash
   ```javascript
   ggg.on('beforeRequest', function(e) {
       try {
           // Your code here
       } catch (err) {
           ggg.log('Error: ' + err);
       }
       return true;
   });
   ```

4. **Test Incrementally** - Start simple, add features gradually

5. **Use Numeric Prefixes** - Control script order
   ```
   01_essential.js
   02_optional.js
   99_debug.js
   ```

## Limitations

### Current Implementation

- Scripts run in a sandboxed Deno runtime with controlled access
- Hook execution has timeout limits (configurable, default 30 seconds)
- All hooks are fully implemented and production-ready

### Future Enhancements

- TypeScript support with automatic type checking
- Script debugging tools and REPL
- More granular URL pattern matching (glob patterns)

## Security

- Scripts run with **full system access** via Deno runtime
- Only load scripts you trust
- Review scripts before enabling
- Scripts can modify downloads, headers, and file operations

## Getting Help

- Check example scripts in `examples/scripts` directory
