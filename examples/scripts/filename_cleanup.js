// Filename Cleanup Script
// Cleans up and normalizes filenames after download completes

ggg.on('completed', function(e) {
    let newName = e.filename;
    let modified = false;

    // Decode URL-encoded characters
    try {
        const decoded = decodeURIComponent(newName);
        if (decoded !== newName) {
            newName = decoded;
            modified = true;
            ggg.log('[Cleanup] Decoded filename: ' + e.filename + ' -> ' + newName);
        }
    } catch (err) {
        // Ignore decoding errors
    }

    // Replace multiple spaces with single space
    const spaceCleaned = newName.replace(/\s+/g, ' ');
    if (spaceCleaned !== newName) {
        newName = spaceCleaned;
        modified = true;
        ggg.log('[Cleanup] Normalized spaces: ' + newName);
    }

    // Remove text in parentheses before extension
    const extMatch = newName.match(/\.[^.]+$/);
    if (extMatch) {
        const ext = extMatch[0];
        const basename = newName.substring(0, newName.length - ext.length);
        const cleaned = basename.replace(/\s*\([^)]*\)\s*$/, '');
        if (cleaned !== basename) {
            newName = cleaned + ext;
            modified = true;
            ggg.log('[Cleanup] Removed parentheses: ' + newName);
        }
    }

    // Trim whitespace
    newName = newName.trim();

    // Apply rename if modified
    if (modified && newName !== e.filename) {
        e.newFilename = newName;
        ggg.log('[Cleanup] Final rename: ' + e.filename + ' -> ' + newName);
    }

    return true;
});

ggg.log('[Filename Cleanup] Script loaded');
