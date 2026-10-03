// Error notification script
// Logs detailed error information

ggg.on('error', function(e) {
    ggg.log('Download failed: ' + e.url);
    ggg.log('Error: ' + e.error);
    ggg.log('Retry count: ' + e.retryCount);

    // Could integrate with external notification systems here
    // For now, just log to console
    return true;
});
