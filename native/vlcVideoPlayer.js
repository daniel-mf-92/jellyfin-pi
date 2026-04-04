/* eslint-disable indent */
/**
 * VLC Video Player — wraps mpvVideoPlayer but routes playback through the VLC backend.
 *
 * Jellyfin's web client supports multiple player plugins. This registers as a
 * separate player with lower priority. When selected (via the "Play in VLC"
 * button), it sets PlayerComponent's backend to 'vlc' before calling load().
 */
(function() {
    // Wait for mpvVideoPlayer to be available
    const waitForMpv = setInterval(() => {
        if (!window._mpvVideoPlayer) return;
        clearInterval(waitForMpv);

        const MpvBase = window._mpvVideoPlayer;

        class vlcVideoPlayer extends MpvBase {
            constructor(options) {
                super(options);
                this.name = 'VLC Video Player';
                this.id = 'vlcvideoplayer';
                // Lower priority than mpv (-1), so mpv is default
                this.priority = -2;
            }

            /**
             * Override play to switch backend to VLC before loading.
             */
            async play(options) {
                console.log('[VLC] Setting backend to vlc');
                window.api.player.setBackend('vlc');
                return await super.play(options);
            }

            /**
             * Override stop to switch backend back to mpv after VLC playback.
             */
            stop(destroyPlayer) {
                const result = super.stop(destroyPlayer);
                console.log('[VLC] Restoring backend to mpv');
                window.api.player.setBackend('mpv');
                return result;
            }

            /**
             * Override destroy to ensure backend is restored.
             */
            destroy() {
                window.api.player.setBackend('mpv');
                return super.destroy();
            }
        }

        window._vlcVideoPlayer = vlcVideoPlayer;
        console.log('[VLC] vlcVideoPlayer plugin registered');
    }, 100);
})();
