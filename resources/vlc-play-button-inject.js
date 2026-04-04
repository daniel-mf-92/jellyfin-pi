/**
 * vlc-play-button-inject.js
 *
 * Injected into the Jellyfin web client via QtWebEngine. Adds a "VLC" play
 * button next to the standard Play button on item detail pages.
 *
 * When clicked, it switches the active backend to VLC and triggers playback.
 */
(function() {
    'use strict';

    const VLC_BTN_CLASS = 'jmp-vlc-play-btn';
    const VLC_COLOR = '#ff8800'; // VLC orange

    function createVlcButton(originalBtn) {
        // Don't duplicate
        if (originalBtn.parentElement &&
            originalBtn.parentElement.querySelector('.' + VLC_BTN_CLASS)) {
            return null;
        }

        const vlcBtn = originalBtn.cloneNode(true);
        vlcBtn.classList.add(VLC_BTN_CLASS);

        // Change the label
        const textSpan = vlcBtn.querySelector('.button-text, span:not(.material-icons)');
        if (textSpan) {
            textSpan.textContent = 'VLC';
        } else {
            // Fallback: append text
            vlcBtn.setAttribute('title', 'Play in VLC');
        }

        // Style it distinctly
        vlcBtn.style.borderColor = VLC_COLOR;
        vlcBtn.style.color = VLC_COLOR;
        vlcBtn.style.marginLeft = '8px';

        // Override click: set backend to VLC, then trigger original play
        vlcBtn.addEventListener('click', function(e) {
            e.preventDefault();
            e.stopPropagation();
            console.log('[VLC-inject] Play in VLC clicked');

            // Set backend to VLC
            if (window.api && window.api.player) {
                window.api.player.setBackend('vlc');
            }

            // Trigger the original button's click
            originalBtn.click();

            // Reset backend after a short delay (in case play didn't work)
            setTimeout(() => {
                // Don't reset if playback started (stop/destroy will reset)
            }, 2000);
        });

        return vlcBtn;
    }

    function injectButtons() {
        // Find play buttons in the Jellyfin UI
        const selectors = [
            'button.btnPlay',
            'button[data-action="play"]',
            '.detailButtons button.playmenu',
            '.mainDetailButtons button[is="emby-playstatebutton"]',
        ];

        for (const sel of selectors) {
            const buttons = document.querySelectorAll(sel);
            for (const btn of buttons) {
                const vlcBtn = createVlcButton(btn);
                if (vlcBtn) {
                    btn.parentElement.insertBefore(vlcBtn, btn.nextSibling);
                }
            }
        }

        // Also look for the resume button
        const resumeButtons = document.querySelectorAll('button.btnResume, button[data-action="resume"]');
        for (const btn of resumeButtons) {
            const vlcBtn = createVlcButton(btn);
            if (vlcBtn) {
                const textSpan = vlcBtn.querySelector('.button-text, span:not(.material-icons)');
                if (textSpan) textSpan.textContent = 'Resume (VLC)';
                btn.parentElement.insertBefore(vlcBtn, btn.nextSibling);
            }
        }
    }

    // Observe DOM changes to inject buttons on page navigation
    const observer = new MutationObserver(function() {
        // Debounce
        clearTimeout(observer._timeout);
        observer._timeout = setTimeout(injectButtons, 500);
    });

    // Start observing once the document is ready
    function init() {
        observer.observe(document.body, {
            childList: true,
            subtree: true,
        });
        // Initial injection
        setTimeout(injectButtons, 1000);
    }

    if (document.readyState === 'complete' || document.readyState === 'interactive') {
        init();
    } else {
        document.addEventListener('DOMContentLoaded', init);
    }

    console.log('[VLC-inject] VLC play button injector loaded');
})();
