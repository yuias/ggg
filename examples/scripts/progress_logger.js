// Progress logger script
// Logs download progress milestones

ggg.on('progress', function(e) {
    if (e.total) {
        const percent = (e.downloaded / e.total * 100).toFixed(1);
        const speedMB = (e.speed / 1024 / 1024).toFixed(2);

        // Log at 25%, 50%, 75%, 100% milestones
        if (percent == '25.0' || percent == '50.0' || percent == '75.0' || percent == '100.0') {
            ggg.log('Progress: ' + percent + '% - ' + speedMB + ' MB/s - ' + e.url);
        }
    }
    return true;
});
