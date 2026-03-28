const jmpInfo = JSON.parse(window.atob("@@data@@"));
window.jmpInfo = jmpInfo;

const features = [
    "filedownload",
    "displaylanguage",
    "htmlaudioautoplay",
    "htmlvideoautoplay",
    "externallinks",
    "clientsettings",
    "multiserver",
    "exitmenu",
    "remotecontrol",
    "fullscreenchange",
    "filedownload",
    "remotevideo",
    "displaymode",
    "screensaver",
    "fileinput"
];

// Detect VLC mode: PlayerComponent checks JMP_EXTERNAL_PLAYER env var.
// The C++ side injects this into jmpInfo so JS can detect it too.
const vlcMode = (function() {
    // Check if settings indicate VLC external player
    // The enableMPV setting being false combined with external player = VLC mode
    const mpvEnabled = jmpInfo.settings?.main?.enableMPV !== false;
    // Also check if jmpInfo has an externalPlayer hint (set by C++ from env var)
    const extPlayer = jmpInfo.externalPlayer || '';
    if (extPlayer.toLowerCase().indexOf('vlc') >= 0) {
        return true;
    }
    // Fallback: if MPV is explicitly disabled, assume VLC/external mode
    if (!mpvEnabled) {
        return true;
    }
    return false;
})();

const plugins = [
    'mpvVideoPlayer',
    'mpvAudioPlayer',
    'jmpInputPlugin',
    'jmpUpdatePlugin',
    'skipIntroPlugin'
];

// Plugins are bundled, return class directly
// Plugin JS files are inlined by C++ getNativeShellScript() to avoid CORS issues.
// The inlined scripts define window["_" + pluginName], so we just reference them directly.
for (const plugin of plugins) {
    window[plugin] = () => {
        return window["_" + plugin];
    };
}

window.NativeShell = {
    openUrl(url, target) {
        window.api.system.openExternalUrl(url);
    },

    downloadFile(downloadInfo) {
        window.api.system.openExternalUrl(downloadInfo.url);
    },

    openClientSettings() {
        showSettingsModal();
    },

    getPlugins() {
        return plugins;
    }
};

function getDeviceProfile() {
    // VLC mode: configure for maximum DirectPlay, no transcoding.
    // VLC can decode virtually everything natively, so we tell the
    // Jellyfin server to send the original stream without transcoding.
    if (vlcMode) {
        return getVlcDeviceProfile();
    }

    return getMpvDeviceProfile();
}

/**
 * VLC device profile: prefer DirectPlay for everything.
 * No codec restrictions, unlimited bitrate, no HLS transcoding needed.
 */
function getVlcDeviceProfile() {
    return {
        'Name': 'Jellyfin Desktop (VLC)',
        'MaxStaticBitrate': 1000000000,
        'MaxStreamingBitrate': 1000000000,
        'MusicStreamingTranscodingBitrate': 1280000,
        'TimelineOffsetSeconds': 5,
        'TranscodingProfiles': [
            // Minimal transcoding profiles as fallback only.
            // VLC handles nearly all formats, so the server should
            // rarely need to transcode.
            { 'Type': 'Audio' },
            {
                'Container': 'ts',
                'Type': 'Video',
                'Protocol': 'hls',
                'AudioCodec': 'aac,mp3,ac3,eac3,opus,flac,vorbis,dts',
                'VideoCodec': 'h264,h265,hevc,mpeg4,mpeg2video,vp9,av1',
                'MaxAudioChannels': '8'
            },
            { 'Container': 'jpeg', 'Type': 'Photo' }
        ],
        // DirectPlay everything: video, audio, photos
        'DirectPlayProfiles': [
            {
                'Type': 'Video',
                // VLC supports essentially all containers/codecs
                'AudioCodec': 'aac,mp3,ac3,eac3,dts,flac,opus,vorbis,pcm,truehd,dca',
                'VideoCodec': 'h264,h265,hevc,mpeg4,mpeg2video,vp8,vp9,av1,vc1,wmv3'
            },
            { 'Type': 'Audio' },
            { 'Type': 'Photo' }
        ],
        'ResponseProfiles': [],
        'ContainerProfiles': [],
        // No codec restrictions in VLC mode — it handles everything
        'CodecProfiles': [],
        'SubtitleProfiles': [
            { 'Format': 'srt', 'Method': 'External' },
            { 'Format': 'srt', 'Method': 'Embed' },
            { 'Format': 'ass', 'Method': 'External' },
            { 'Format': 'ass', 'Method': 'Embed' },
            { 'Format': 'sub', 'Method': 'Embed' },
            { 'Format': 'sub', 'Method': 'External' },
            { 'Format': 'ssa', 'Method': 'Embed' },
            { 'Format': 'ssa', 'Method': 'External' },
            { 'Format': 'smi', 'Method': 'Embed' },
            { 'Format': 'smi', 'Method': 'External' },
            { 'Format': 'pgssub', 'Method': 'Embed' },
            { 'Format': 'dvdsub', 'Method': 'Embed' },
            { 'Format': 'dvbsub', 'Method': 'Embed' },
            { 'Format': 'pgs', 'Method': 'Embed' },
            { 'Format': 'vobsub', 'Method': 'Embed' },
            { 'Format': 'idx', 'Method': 'External' }
        ]
    };
}

/**
 * Original mpv device profile with all the existing transcode/codec settings.
 */
function getMpvDeviceProfile() {
    const CodecProfiles = [];

    if (jmpInfo.settings.video.force_transcode_dovi) {
        CodecProfiles.push({
            'Type': 'Video',
            'Conditions': [
                {
                    'Condition': 'NotEquals',
                    'Property': 'VideoRangeType',
                    'Value': 'DOVI'
                }
            ]
        });
    }

    if (jmpInfo.settings.video.force_transcode_hdr) {
        CodecProfiles.push({
            'Type': 'Video',
            'Conditions': [
                {
                    'Condition': 'Equals',
                    'Property': 'VideoRangeType',
                    'Value': 'SDR'
                }
            ]
        });
    }

    if (jmpInfo.settings.video.force_transcode_hi10p) {
        CodecProfiles.push({
            'Type': 'Video',
            'Conditions': [
                {
                    'Condition': 'LessThanEqual',
                    'Property': 'VideoBitDepth',
                    'Value': '8',
                }
            ]
        });
    }

    if (jmpInfo.settings.video.force_transcode_hevc) {
        CodecProfiles.push({
            'Type': 'Video',
            'Codec': 'hevc',
            'Conditions': [
                {
                    'Condition': 'Equals',
                    'Property': 'Width',
                    'Value': '0',
                }
            ],
        });
        CodecProfiles.push({
            'Type': 'Video',
            'Codec': 'h265',
            'Conditions': [
                {
                    'Condition': 'Equals',
                    'Property': 'Width',
                    'Value': '0',
                }
            ],
        });
    }

    if (jmpInfo.settings.video.force_transcode_av1) {
        CodecProfiles.push({
            'Type': 'Video',
            'Codec': 'av1',
            'Conditions': [
                {
                    'Condition': 'Equals',
                    'Property': 'Width',
                    'Value': '0',
                }
            ],
        });
    }

    if (jmpInfo.settings.video.force_transcode_4k) {
        CodecProfiles.push({
            'Type': 'Video',
            'Conditions': [
                {
                    'Condition': 'LessThanEqual',
                    'Property': 'Width',
                    'Value': '1920',
                },
                {
                    'Condition': 'LessThanEqual',
                    'Property': 'Height',
                    'Value': '1080',
                }
            ]
        });
    }

    const DirectPlayProfiles = [{ 'Type': 'Audio' }, { 'Type': 'Photo' }];

    if (!jmpInfo.settings.video.always_force_transcode) {
        DirectPlayProfiles.push({ 'Type': 'Video' });
    }

    return {
        'Name': 'Jellyfin Desktop',
        'MaxStaticBitrate': 1000000000,
        'MusicStreamingTranscodingBitrate': 1280000,
        'TimelineOffsetSeconds': 5,
        'TranscodingProfiles': [
            { 'Type': 'Audio' },
            {
                'Container': 'ts',
                'Type': 'Video',
                'Protocol': 'hls',
                'AudioCodec': 'aac,mp3,ac3,opus,vorbis',
                'VideoCodec': jmpInfo.settings.video.allow_transcode_to_hevc
                    ? (
                        jmpInfo.settings.video.prefer_transcode_to_h265
                            ? 'h265,hevc,h264,mpeg4,mpeg2video'
                            : 'h264,h265,hevc,mpeg4,mpeg2video'
                    )
                    : 'h264,mpeg4,mpeg2video',
                'MaxAudioChannels': jmpInfo.settings.audio.channels === "2.0" ? '2' : '6'
            },
            { 'Container': 'jpeg', 'Type': 'Photo' }
        ],
        DirectPlayProfiles,
        'ResponseProfiles': [],
        'ContainerProfiles': [],
        CodecProfiles,
        'SubtitleProfiles': [
            { 'Format': 'srt', 'Method': 'External' },
            { 'Format': 'srt', 'Method': 'Embed' },
            { 'Format': 'ass', 'Method': 'External' },
            { 'Format': 'ass', 'Method': 'Embed' },
            { 'Format': 'sub', 'Method': 'Embed' },
            { 'Format': 'sub', 'Method': 'External' },
            { 'Format': 'ssa', 'Method': 'Embed' },
            { 'Format': 'ssa', 'Method': 'External' },
            { 'Format': 'smi', 'Method': 'Embed' },
            { 'Format': 'smi', 'Method': 'External' },
            { 'Format': 'pgssub', 'Method': 'Embed' },
            { 'Format': 'dvdsub', 'Method': 'Embed' },
            { 'Format': 'dvbsub', 'Method': 'Embed' },
            { 'Format': 'pgs', 'Method': 'Embed' }
        ]
    };
}

async function createApi() {
    // Can't append script until document exists
    await new Promise(resolve => {
        document.addEventListener('DOMContentLoaded', resolve);
    });

    const channel = await new Promise((resolve) => {
        /*global QWebChannel */
        new QWebChannel(window.qt.webChannelTransport, resolve);
    });
    return channel.objects;
}

const sectionsFromStorage = window.sessionStorage.getItem('sections');
if (sectionsFromStorage) {
    jmpInfo.sections = JSON.parse(sectionsFromStorage);
}

let rawSettings = {};
Object.assign(rawSettings, jmpInfo.settings);
const settingsFromStorage = window.sessionStorage.getItem('settings');
if (settingsFromStorage) {
    rawSettings = JSON.parse(settingsFromStorage);
    Object.assign(jmpInfo.settings, rawSettings);
}

const settingsDescriptionsFromStorage = window.sessionStorage.getItem('settingsDescriptions');
if (settingsDescriptionsFromStorage) {
    jmpInfo.settingsDescriptions = JSON.parse(settingsDescriptionsFromStorage);
}

jmpInfo.settingsDescriptionsUpdate = [];
jmpInfo.settingsUpdate = [];

// Expose VLC mode flag globally for other scripts to check
window.jmpVlcMode = vlcMode;
if (vlcMode) {
    console.log('[JMP] VLC mode active — DirectPlay preferred, no transcoding restrictions');
}

window.apiPromise = createApi();
window.initCompleted = new Promise(async (resolve) => {
    window.api = await window.apiPromise;
    const settingUpdate = (section, key) => (
        (data) => new Promise(resolve => {
            rawSettings[section][key] = data;
            window.sessionStorage.setItem("settings", JSON.stringify(rawSettings));
            window.api.settings.setValue(section, key, data, resolve);
        })
    );
    const setSetting = (section, key) => {
        Object.defineProperty(jmpInfo.settings[section], key, {
            set: settingUpdate(section, key),
            get: () => rawSettings[section][key]
        });
    };
    for (const settingGroup of Object.keys(rawSettings)) {
        jmpInfo.settings[settingGroup] = {};
        for (const setting of Object.keys(rawSettings[settingGroup])) {
            setSetting(settingGroup, setting, jmpInfo.settings[settingGroup][setting]);
        }
    }
    window.api.settings.sectionValueUpdate.connect(
        (section, data) => {
            Object.assign(rawSettings[section], data);
            for (const callback of jmpInfo.settingsUpdate) {
                try {
                    callback(section, data);
                } catch (e) {
                    console.error("Update handler failed:", e);
                }
            }

            // Settings will be outdated if page reloads, so save them to session storage
            window.sessionStorage.setItem("settings", JSON.stringify(rawSettings));
        }
    );
    window.api.settings.groupUpdate.connect(
        (section, data) => {
            jmpInfo.settingsDescriptions[section] = data.settings;
            for (const callback of jmpInfo.settingsDescriptionsUpdate) {
                try {
                    callback(section, data);
                } catch (e) {
                    console.error("Description update handler failed:", e);
                }
            }

            // Settings will be outdated if page reloads, so save them to session storage
            window.sessionStorage.setItem("settingsDescriptions", JSON.stringify(jmpInfo.settingsDescriptions));
        }
    );

    // Sync cursor visibility with jellyfin-web's mouse idle state
    const observer = new MutationObserver((mutations) => {
        for (const mutation of mutations) {
            if (mutation.attributeName === 'class') {
                const isIdle = document.body.classList.contains('mouseIdle');
                if (window.api && window.api.window) window.api.window.setCursorVisibility(!isIdle);
            }
        }
    });
    observer.observe(document.body, { attributes: true, attributeFilter: ['class'] });

    resolve();
});

window.NativeShell.AppHost = {
    init() {
        return Promise.resolve({
            deviceName: jmpInfo.deviceName,
            appName: "Jellyfin Desktop",
            appVersion: jmpInfo.version
        });
    },
    getDefaultLayout() {
        return jmpInfo.mode;
    },
    supports(command) {
        return features.includes(command.toLowerCase());
    },
    getDeviceProfile,
    getSyncProfile: getDeviceProfile,
    appName() {
        return "Jellyfin Desktop";
    },
    appVersion() {
        return jmpInfo.version;
    },
    deviceName() {
        return jmpInfo.deviceName;
    },
    exit() {
        window.api.system.exit();
    }
};

async function showSettingsModal() {
    await initCompleted;

    const tooltipCSS = `
        .tooltip {
            position: relative;
            display: inline-block;
            margin-left: 0.5rem;
            font-size: 18px;
            vertical-align: sub;
        }

        .tooltip .tooltip-text {
            visibility: hidden;
            width: max-content;
            max-width: 40em;
            background-color: black;
            color: white;
            text-align: left;
            position: absolute;
            z-index: 1;
            border-radius: 6px;
            padding: 5px;
            top: -4px;
            left: 25px;
            border: solid 1px grey;
            font-size: 12px;
        }

        .tooltip:hover .tooltip-text {
            visibility: visible;
        }`;

    var style = document.createElement('style')
    style.innerText = tooltipCSS
    document.head.appendChild(style)

    const modalContainer = document.createElement("div");
    modalContainer.className = "dialogContainer";
    modalContainer.style.backgroundColor = "rgba(0,0,0,0.5)";
    modalContainer.addEventListener("click", e => {
        if (e.target == modalContainer) {
            modalContainer.remove();
        }
    });
    document.body.appendChild(modalContainer);

    const modalContainer2 = document.createElement("div");
    modalContainer2.className = "focuscontainer dialog dialog-fixedSize dialog-small formDialog opened";
    modalContainer.appendChild(modalContainer2);

    const modalHeader = document.createElement("div");
    modalHeader.className = "formDialogHeader";
    modalContainer2.appendChild(modalHeader);

    const title = document.createElement("h3");
    title.className = "formDialogHeaderTitle";
    title.textContent = "Client Settings";
    modalHeader.appendChild(title);

    const modalContents = document.createElement("div");
    modalContents.className = "formDialogContent smoothScrollY";
    modalContents.style.paddingTop = "2em";
    modalContents.style.marginBottom = "6.2em";
    modalContainer2.appendChild(modalContents);

    // VLC mode indicator at top of settings
    if (vlcMode) {
        const vlcBanner = document.createElement("div");
        vlcBanner.style.cssText = "background:#1a5276;color:#fff;padding:10px 16px;border-radius:6px;margin:0 16px 16px;font-size:14px;";
        vlcBanner.innerHTML = "<strong>VLC Mode Active</strong> — Video plays in external VLC window. " +
            "DirectPlay is preferred; transcoding settings below are ignored.";
        modalContents.appendChild(vlcBanner);
    }

    const settingUpdateHandlers = {};
    for (const sectionOrder of jmpInfo.sections.sort((a, b) => a.order - b.order)) {
        const section = sectionOrder.key;
        const group = document.createElement("fieldset");
        group.className = "editItemMetadataForm editMetadataForm dialog-content-centered";
        group.style.border = 0;
        group.style.outline = 0;
        modalContents.appendChild(group);

        const createSection = async (clear) => {
            if (clear) {
                group.innerHTML = "";
            }

            const values = jmpInfo.settings[section];
            const settings = jmpInfo.settingsDescriptions[section];

            const legend = document.createElement("legend");
            const legendHeader = document.createElement("h2");
            legendHeader.textContent = section;
            legendHeader.style.textTransform = "capitalize";
            legend.appendChild(legendHeader);
            if (section == "other") {
                const legendSubHeader = document.createElement("h4");
                legendSubHeader.textContent = "Use this section to input custom MPV configuration. These will override the above settings.";
                legend.appendChild(legendSubHeader);
            }
            group.appendChild(legend);

            for (const setting of settings) {
                const label = document.createElement("label");
                label.className = "inputContainer";
                label.style.marginBottom = "1.8em";
                label.style.display = "block";

                // In VLC mode, dim video transcoding settings since they don't apply
                if (vlcMode && section === 'video') {
                    label.style.opacity = '0.5';
                    label.title = 'Not applicable in VLC mode';
                }

                let helpElement;
                if (setting.help) {
                    helpElement = document.createElement("div");
                    helpElement.className = "tooltip";
                    const helpIcon = document.createElement("span");
                    helpIcon.style.fontSize = "18px"
                    helpIcon.className = "material-icons help_outline";
                    helpElement.appendChild(helpIcon);
                    const tooltipElement = document.createElement("span");
                    tooltipElement.className = "tooltip-text";
                    tooltipElement.innerText = setting.help;
                    helpElement.appendChild(tooltipElement);
                }

                if (setting.options) {
                    const safeValues = {};
                    const control = document.createElement("select");
                    control.className = "emby-select-withcolor emby-select";
                    for (const option of setting.options) {
                        safeValues[String(option.value)] = option.value;
                        const opt = document.createElement("option");
                        opt.value = option.value;
                        opt.selected = option.value == values[setting.key];
                        let optionName = option.title;
                        const swTest = `${section}.${setting.key}.`;
                        const swTest2 = `${section}.`;
                        if (optionName.startsWith(swTest)) {
                            optionName = optionName.substring(swTest.length);
                        } else if (optionName.startsWith(swTest2)) {
                            optionName = optionName.substring(swTest2.length);
                        }
                        opt.appendChild(document.createTextNode(optionName));
                        control.appendChild(opt);
                    }
                    control.addEventListener("change", async (e) => {
                        jmpInfo.settings[section][setting.key] = safeValues[e.target.value];
                    });
                    const labelText = document.createElement('label');
                    labelText.className = "inputLabel";
                    labelText.textContent = (setting.displayName ? setting.displayName : setting.key) + ": ";
                    label.appendChild(labelText);
                    if (helpElement) label.appendChild(helpElement);
                    label.appendChild(control);
                } else if (setting.inputType === "textarea") {
                    const control = document.createElement("textarea");
                    control.className = "emby-select-withcolor emby-select";
                    control.style = "resize: none;"
                    control.value = values[setting.key];
                    control.rows = 5;
                    control.addEventListener("change", e => {
                        jmpInfo.settings[section][setting.key] = e.target.value;
                    });
                    const labelText = document.createElement('label');
                    labelText.className = "inputLabel";
                    labelText.textContent = (setting.displayName ? setting.displayName : setting.key) + ": ";
                    label.appendChild(labelText);
                    if (helpElement) label.appendChild(helpElement);
                    label.appendChild(control);
                } else {
                    const control = document.createElement("input");
                    control.type = "checkbox";
                    control.checked = values[setting.key];
                    control.addEventListener("change", e => {
                        jmpInfo.settings[section][setting.key] = e.target.checked;
                    });
                    label.appendChild(control);
                    label.appendChild(document.createTextNode(" " + (setting.displayName ? setting.displayName : setting.key)));
                    if (helpElement) label.appendChild(helpElement);
                }

                group.appendChild(label);
            }
        };
        settingUpdateHandlers[section] = () => createSection(true);
        createSection();
    }

    const onSectionUpdate = (section) => {
        if (section in settingUpdateHandlers) {
            settingUpdateHandlers[section]();
        }
    };
    jmpInfo.settingsDescriptionsUpdate.push(onSectionUpdate);
    jmpInfo.settingsUpdate.push(onSectionUpdate);

    if (jmpInfo.settings.main.userWebClient) {
        const group = document.createElement("fieldset");
        group.className = "editItemMetadataForm editMetadataForm dialog-content-centered";
        group.style.border = 0;
        group.style.outline = 0;
        modalContents.appendChild(group);
        const legend = document.createElement("legend");
        const legendHeader = document.createElement("h2");
        legendHeader.textContent = "Saved Server";
        legend.appendChild(legendHeader);
        const legendSubHeader = document.createElement("h4");
        legendSubHeader.textContent = (
            "The server you first connected to is your saved server. " +
            "It provides the web client for Jellyfin in the absence of a bundled one. " +
            "You can use this option to change it to another one. This does NOT log you off."
        );
        legend.appendChild(legendSubHeader);
        group.appendChild(legend);

        const resetSavedServer = document.createElement("button");
        resetSavedServer.className = "raised button-cancel block btnCancel emby-button";
        resetSavedServer.textContent = "Reset Saved Server"
        resetSavedServer.style.marginLeft = "auto";
        resetSavedServer.style.marginRight = "auto";
        resetSavedServer.style.maxWidth = "50%";
        resetSavedServer.addEventListener("click", async () => {
            window.jmpInfo.settings.main.userWebClient = '';
            window.location.href = jmpInfo.scriptPath + "/find-webclient.html";
        });
        group.appendChild(resetSavedServer);
    }

    const closeContainer = document.createElement("div");
    closeContainer.className = "formDialogFooter";
    modalContents.appendChild(closeContainer);

    const close = document.createElement("button");
    close.className = "raised button-cancel block btnCancel formDialogFooterItem emby-button";
    close.textContent = "Close"
    close.addEventListener("click", () => {
        modalContainer.remove();
    });
    closeContainer.appendChild(close);
}

let lastFullscreenState = window.jmpInfo.settings.main.fullscreen;

window.jmpInfo.settingsUpdate.push(function(section) {
    if (section === 'main') {
        const currentFullscreenState = window.jmpInfo.settings.main.fullscreen;
        if (currentFullscreenState !== lastFullscreenState) {
            lastFullscreenState = currentFullscreenState;

            if (window.api && window.api.player) {
                window.api.player.notifyFullscreenChange(currentFullscreenState);
                console.log('Player fullscreen notified');
            }

            if (window.Events && window.playbackManager && window.playbackManager._currentPlayer) {
                window.Events.trigger(window.playbackManager._currentPlayer, 'fullscreenchange');
            }
        }
    }
});

// ─── Pi-home-A TV Fixes ───────────────────────────────────────────────────

// Fix: Force single-item scroll on horizontal carousels (Recently Added, etc.)
// Jellyfin web UI scrolls by visible-count per arrow key; override to scroll by 1.
(function() {
    function patchScrollers() {
        document.querySelectorAll('.scrollSlider').forEach(function(slider) {
            if (slider._patched) return;
            slider._patched = true;
            slider.addEventListener('keydown', function(e) {
                if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
                    e.stopPropagation();
                    var cards = slider.querySelectorAll('.card');
                    var focused = document.activeElement;
                    if (!focused || !slider.contains(focused)) return;
                    
                    var idx = Array.from(cards).indexOf(focused.closest('.card'));
                    if (idx < 0) return;
                    
                    var nextIdx = e.key === 'ArrowRight' ? idx + 1 : idx - 1;
                    if (nextIdx >= 0 && nextIdx < cards.length) {
                        var target = cards[nextIdx].querySelector('[tabindex], button, a') || cards[nextIdx];
                        target.focus();
                        target.scrollIntoView({behavior: 'smooth', block: 'nearest', inline: 'center'});
                    }
                    e.preventDefault();
                }
            }, true);
        });
    }
    
    var observer = new MutationObserver(function() { setTimeout(patchScrollers, 500); });
    document.addEventListener('DOMContentLoaded', function() {
        observer.observe(document.body, {childList: true, subtree: true});
        setTimeout(patchScrollers, 2000);
    });
    window.addEventListener('hashchange', function() { setTimeout(patchScrollers, 1000); });
    setTimeout(patchScrollers, 3000);
})();

// Fix: Remove alphabetical letter grouping in library views — show all items
(function() {
    function disableLetterGrouping() {
        var style = document.createElement('style');
        style.textContent = '.sectionTitle.sectionTitle-cards.padded-left ~ .itemsContainer .prefixContainer { display: none !important; }' +
            '.alphabetPicker { display: none !important; }' +
            '.itemsContainer .listPaging { display: none !important; }';
        document.head.appendChild(style);
        
        if (window.ApiClient && window.ApiClient.getCurrentUserId) {
            var userId = window.ApiClient.getCurrentUserId();
            if (userId) {
                ['f137a2dd21bbc1b99aa5c0f6bf02a805', '767bffe4f11c93ef34b805451a696a4e'].forEach(function(libId) {
                    window.ApiClient.getJSON(window.ApiClient.getUrl('DisplayPreferences/' + libId, {
                        userId: userId, client: 'emby'
                    })).then(function(prefs) {
                        if (!prefs.CustomPrefs) prefs.CustomPrefs = {};
                        prefs.CustomPrefs[libId] = JSON.stringify({
                            SortBy: 'SortName',
                            SortOrder: 'Ascending',
                            ViewMode: 'images'
                        });
                        prefs.ScrollDirection = 'Vertical';
                        window.ApiClient.updateDisplayPreferences(libId, prefs, userId, 'emby');
                    }).catch(function(){});
                });
            }
        }
    }
    setTimeout(disableLetterGrouping, 4000);
    window.addEventListener('hashchange', function() { setTimeout(disableLetterGrouping, 2000); });
})();

// Fix: Default to 500 items per page in library views
(function() {
    function setPageSize() {
        if (!window.ApiClient || !window.ApiClient.getCurrentUserId) return;
        var userId = window.ApiClient.getCurrentUserId();
        if (!userId) return;
        
        if (window.userSettings && window.userSettings.libraryPageSize) {
            window.userSettings.libraryPageSize = function() { return 500; };
        }
        
        ['f137a2dd21bbc1b99aa5c0f6bf02a805', '767bffe4f11c93ef34b805451a696a4e'].forEach(function(libId) {
            window.ApiClient.getJSON(window.ApiClient.getUrl('DisplayPreferences/' + libId, {
                userId: userId, client: 'emby'
            })).then(function(prefs) {
                if (!prefs.CustomPrefs) prefs.CustomPrefs = {};
                prefs.CustomPrefs['PageSize'] = '500';
                window.ApiClient.updateDisplayPreferences(libId, prefs, userId, 'emby');
            }).catch(function(){});
        });
    }
    setTimeout(setPageSize, 4000);
    window.addEventListener('hashchange', function() { setTimeout(setPageSize, 2000); });
})();


// Feature: Random unwatched media button on home page
(function() {
    var DICE_SVG = '<svg viewBox="0 0 24 24" width="48" height="48" fill="white"><path d="M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zM7.5 18c-.83 0-1.5-.67-1.5-1.5S6.67 15 7.5 15s1.5.67 1.5 1.5S8.33 18 7.5 18zm0-9C6.67 9 6 8.33 6 7.5S6.67 6 7.5 6 9 6.67 9 7.5 8.33 9 7.5 9zm4.5 4.5c-.83 0-1.5-.67-1.5-1.5s.67-1.5 1.5-1.5 1.5.67 1.5 1.5-.67 1.5-1.5 1.5zm4.5 4.5c-.83 0-1.5-.67-1.5-1.5s.67-1.5 1.5-1.5 1.5.67 1.5 1.5-.67 1.5-1.5 1.5zm0-9c-.83 0-1.5-.67-1.5-1.5S15.67 6 16.5 6s1.5.67 1.5 1.5S17.33 9 16.5 9z"/></svg>';

    function addRandomButton() {
        if (document.getElementById("jmp-random-btn")) return;
        if (!window.location.hash.includes("home")) return;
        if (!window.ApiClient) return;

        var btn = document.createElement("button");
        btn.id = "jmp-random-btn";
        btn.innerHTML = DICE_SVG + "<span style='display:block;font-size:14px;margin-top:4px'>Random</span>";
        btn.style.cssText = "position:fixed;bottom:30px;right:30px;z-index:9999;background:rgba(100,60,180,0.85);border:2px solid rgba(255,255,255,0.3);border-radius:16px;padding:12px 18px;cursor:pointer;color:white;text-align:center;backdrop-filter:blur(10px);transition:transform 0.2s;";
        btn.tabIndex = 0;

        btn.addEventListener("focus", function() { btn.style.transform = "scale(1.15)"; btn.style.borderColor = "#e94560"; });
        btn.addEventListener("blur", function() { btn.style.transform = "scale(1)"; btn.style.borderColor = "rgba(255,255,255,0.3)"; });
        btn.addEventListener("click", playRandom);
        btn.addEventListener("keydown", function(e) { if (e.key === "Enter") { e.preventDefault(); playRandom(); } });

        document.body.appendChild(btn);
    }

    async function playRandom() {
        if (!window.ApiClient) return;
        var userId = window.ApiClient.getCurrentUserId();
        if (!userId) return;
        try {
            var url = window.ApiClient.getUrl("Users/" + userId + "/Items", {
                SortBy: "Random", Limit: 1, Recursive: true,
                IncludeItemTypes: "Movie,Episode", Filters: "IsUnplayed", Fields: "Overview,Path"
            });
            var result = await window.ApiClient.getJSON(url);
            if (result.Items && result.Items.length > 0) {
                var item = result.Items[0];
                console.log("Random pick: " + item.Name + " (" + item.Type + ")");
                if (window.playbackManager) {
                    window.playbackManager.play({ items: [item], startPositionTicks: 0 });
                } else {
                    window.location.hash = "#!/details?id=" + item.Id + "&serverId=" + item.ServerId;
                }
            }
        } catch(e) { console.log("Random play error: " + e); }
    }

    setTimeout(addRandomButton, 3000);
    window.addEventListener("hashchange", function() {
        var old = document.getElementById("jmp-random-btn");
        if (old) old.remove();
        setTimeout(addRandomButton, 1500);
    });
    var obs = new MutationObserver(function() { setTimeout(addRandomButton, 500); });
    if (document.body) obs.observe(document.body, {childList: true, subtree: true});
    else document.addEventListener("DOMContentLoaded", function() { obs.observe(document.body, {childList: true, subtree: true}); });
})();


// Fix: Block native arrow key handling in web view.
// JMP's InputComponent already processes arrow keys via hostInput/inputPlugin.
// Without this, both InputComponent AND the native web view handle arrows = double-skip.
(function() {
    document.addEventListener("keydown", function(e) {
        if (e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            e.stopPropagation();
        }
    }, true);
    document.addEventListener("keyup", function(e) {
        if (e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            e.stopPropagation();
        }
    }, true);
})();
