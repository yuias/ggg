// Pixiv Download Helper
// Automatically sets Referer and User-Agent for Pixiv downloads

ggg.on('beforeRequest', function(e) {
    const url = e.url;

    // Check if this is a Pixiv or pximg URL
    if (url.includes('pixiv') || url.includes('pximg')) {
        ggg.log('[Pixiv] Configuring download for: ' + url);

        // Set Referer header (required for Pixiv CDN)
        e.headers['Referer'] = 'https://www.pixiv.net/';

        // Set User-Agent to avoid blocks
        e.userAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36';

        ggg.log('[Pixiv] Headers configured for ' + url);
    }

    return true;
}, 'pxi'); // Filter to only run for URLs containing 'pxi' (pixiv.net, pximg.net)

ggg.log('[Pixiv Helper] Script loaded');
