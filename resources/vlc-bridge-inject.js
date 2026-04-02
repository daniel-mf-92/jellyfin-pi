(function() {
    'use strict';
    console.log('[VLC-BRIDGE] Script injected at document start');

    // Hide all video elements with CSS
    var hideVideoStyle = document.createElement('style');
    hideVideoStyle.id = 'vlc-bridge-hide-video';
    hideVideoStyle.textContent = 'video { visibility: hidden !important; opacity: 0 !important; }';
    
    function injectHideStyle() {
        if (document.head && !document.getElementById('vlc-bridge-hide-video')) {
            document.head.appendChild(hideVideoStyle);
            console.log('[VLC-BRIDGE] CSS injected - videos hidden');
        }
    }
    
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', injectHideStyle);
    } else {
        injectHideStyle();
    }

    // Hook HTMLMediaElement.prototype.src setter
    const origSrcDescriptor = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'src');
    if (origSrcDescriptor && origSrcDescriptor.set) {
        Object.defineProperty(HTMLMediaElement.prototype, 'src', {
            set: function(value) {
                if (value && (value.includes('/Videos/') || value.includes('/video'))) {
                    console.log('VLC_PLAY_SRCSET:' + value);
                    this.pause();
                    this.autoplay = false;
                    return;
                }
                origSrcDescriptor.set.call(this, value);
            },
            get: origSrcDescriptor.get,
            configurable: true
        });
        console.log('[VLC-BRIDGE] Hooked HTMLMediaElement.src setter');
    }

    // Hook setAttribute for 'src'
    const origSetAttribute = Element.prototype.setAttribute;
    Element.prototype.setAttribute = function(name, value) {
        if (this.tagName === 'VIDEO' && name === 'src' && value && (value.includes('/Videos/') || value.includes('/video'))) {
            console.log('VLC_PLAY_SETATTR:' + value);
            this.pause();
            this.autoplay = false;
            return;
        }
        return origSetAttribute.call(this, name, value);
    };
    console.log('[VLC-BRIDGE] Hooked setAttribute');

    // Hook play() method
    const origPlay = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = function() {
        var src = this.currentSrc || this.src;
        if (src && (src.includes('/Videos/') || src.includes('/video'))) {
            console.log('VLC_PLAY_METHOD:' + src);
            this.pause();
            return Promise.reject(new DOMException('Intercepted by VLC bridge', 'NotAllowedError'));
        }
        return origPlay.call(this);
    };
    console.log('[VLC-BRIDGE] Hooked play() method');

    console.log('[VLC-BRIDGE] All hooks installed successfully');
})();
