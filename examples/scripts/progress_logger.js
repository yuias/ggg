// Progress logger script
// Logs each 25/50/75% milestone once, plus a final line on completion.
// Progress updates are throttled, so exact values are rarely hit; the script
// tracks the highest milestone already crossed instead.

(function () {
    var MILESTONES = [25, 50, 75];
    // Highest milestone logged so far, keyed by URL (progress events carry no id).
    var lastMilestone = {};

    ggg.on('progress', function (e) {
        if (typeof e.percentage !== 'number') {
            return true;
        }

        var crossed = 0;
        for (var i = 0; i < MILESTONES.length; i++) {
            if (MILESTONES[i] <= e.percentage) {
                crossed = MILESTONES[i];
            }
        }

        if (crossed > (lastMilestone[e.url] || 0)) {
            lastMilestone[e.url] = crossed;
            var speedMB = ((e.speed || 0) / 1024 / 1024).toFixed(2);
            ggg.log('Progress: ' + crossed + '% - ' + speedMB + ' MB/s - ' + e.url);
        }
        return true;
    });

    ggg.on('completed', function (e) {
        ggg.log('Progress: 100% - ' + e.filename + ' (' + e.size + ' bytes)');
        delete lastMilestone[e.url];
        return true;
    });

    ggg.on('error', function (e) {
        delete lastMilestone[e.url];
        return true;
    });
})();
