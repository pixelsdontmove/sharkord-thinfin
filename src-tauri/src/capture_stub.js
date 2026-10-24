// runs in the webview before any page script (outside the site's csp).
// three jobs:
//   1. forward page console warnings and errors to the rust console.
//   2. render the chrome panels (add server, settings) that the bar webview
//      requests, because the 36px bar strip has no room for text input.
//   3. react to the global keybind events (chrome:hotkey) emitted by the rust
//      input hook, clicking the hosted client's own voice buttons.
// screen share uses the browser's own getDisplayMedia (web streaming).
(function () {
  'use strict';

  // top document only. rust writes each listener's registry entry into the
  // top document (ExecuteScript), but the callback id lives in the document
  // that registered it; a listener registered from an iframe becomes a
  // phantom entry that warns "Couldn't find callback id" on every event.
  // the client's own iframes need none of this stub.
  if (window.self !== window.top) return;

  // the dark shell page only paints one frame and hands off to the hosted
  // client; it needs none of this. bailing here also stops a real bug: the
  // shell's in-flight listen registrations land in the client document after
  // the hand-off (ExecuteScript runs in the current top document), leaving
  // phantom registry entries whose callback ids died with the shell document
  // - every event then warns "Couldn't find callback id".
  // note: the custom protocol is served at http://sharkordshell.localhost,
  // so the protocol is "http:" and the hostname is what identifies it.
  if (
    location.protocol === 'sharkordshell:'
    || location.hostname === 'sharkordshell.localhost'
  ) {
    return;
  }

  // chromium paints a blank document white, so a loading or still-blank page
  // flashes white before the client renders. the init script runs before any
  // page script, so paint the document dark from the first instant; drop the
  // override shortly after load so the app's own background takes over.
  var shellDark = '#1e1f22';
  // the document background is white until the app paints its own, and the
  // first paint can land before any dom element exists, so adopt a dark
  // stylesheet at document start (needs no dom); the inline styles below
  // cover the dom phase as a fallback.
  var darkSheet = null;
  try {
    darkSheet = new CSSStyleSheet();
    darkSheet.replaceSync(
      'html, body { background-color: ' + shellDark + ' !important; }'
    );
    document.adoptedStyleSheets = [darkSheet];
  } catch (e) {
    // unsupported: the inline styles below still cover the dom phase.
  }
  function paintShellDark() {
    var root = document.documentElement;
    if (root) root.style.backgroundColor = shellDark;
    if (document.body) document.body.style.backgroundColor = shellDark;
  }
  // runs at document start, when the document element may not exist yet.
  paintShellDark();
  document.addEventListener('DOMContentLoaded', paintShellDark);
  window.addEventListener('load', function () {
    setTimeout(function () {
      if (darkSheet) document.adoptedStyleSheets = [];
      var root = document.documentElement;
      if (root) root.style.backgroundColor = '';
      if (document.body) document.body.style.backgroundColor = '';
    }, 500);
  });

  var tag = '[sharkord-chrome]';

  function pageLog(level, args) {
    try {
      var internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== 'function') {
        return;
      }
      var parts = [];
      for (var i = 0; i < args.length; i++) {
        var arg = args[i];
        if (typeof arg === 'object' && arg !== null) {
          try {
            parts.push(JSON.stringify(arg));
          } catch (error) {
            parts.push(String(arg));
          }
        } else {
          parts.push(String(arg));
        }
      }
      internals.invoke('capture_log', {
        level: level,
        message: parts.join(' ')
      });
    } catch (error) {
      // never let the bridge break the page
    }
  }

  var originalWarn = console.warn;
  var originalError = console.error;
  console.warn = function () {
    originalWarn.apply(console, arguments);
    pageLog('warn', Array.prototype.slice.call(arguments));
  };
  console.error = function () {
    originalError.apply(console, arguments);
    pageLog('error', Array.prototype.slice.call(arguments));
  };

  var internals = window.__TAURI_INTERNALS__;
  if (!internals || typeof internals.invoke !== 'function') {
    return;
  }
  function invoke(cmd, args) {
    return internals.invoke(cmd, args || {});
  }

  // a page reload orphans the previous life's capture session (its stop
  // trigger was a track listener on the dead page), so kill any capture that
  // is still running. a fresh app start is a no-op.
  invoke('system_audio_stop').catch(function () {});

  // --- screen share pill + system audio ---------------------------------
  // when the hosted client starts a getDisplayMedia capture, webview2 shows a
  // "X is sharing your screen" pill. we hide it at the win32 level the moment
  // the capture starts (the promise resolves), with a short burst to catch it
  // as it renders.
  //
  // webview2's own "share with system audio" loopback fails
  // (NotReadableError: Could not start audio source), so the audio
  // constraint is always stripped from the picker (no checkbox, one picker)
  // and a speaker toggle in the bar decides instead. when it is on we attach
  // our own system audio track: rust captures the default render device with
  // a plain wasapi loopback and streams the pcm here as base64 chunks, which
  // we feed into a script processor node exposed as a mediastream track.
  (function () {
    var md = navigator.mediaDevices;
    if (!md || typeof md.getDisplayMedia !== 'function') return;
    var orig = md.getDisplayMedia.bind(md);

    var sysAudio = {
      context: null,
      processor: null,
      ring: null
    };

    function b64ToFloat32(b64) {
      var bin = atob(b64);
      var bytes = new Uint8Array(bin.length);
      for (var i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      return new Float32Array(bytes.buffer, bytes.byteOffset, bytes.length / 4);
    }

    function ringPush(f32) {
      var ring = sysAudio.ring;
      var n = f32.length >> 1;
      for (var i = 0; i < n; i++) {
        ring.left[ring.write] = f32[i * 2];
        ring.right[ring.write] = f32[i * 2 + 1];
        ring.write = (ring.write + 1) % ring.size;
        if (ring.count < ring.size) {
          ring.count++;
        } else {
          // full: drop the oldest sample
          ring.read = (ring.read + 1) % ring.size;
        }
      }
    }

    function ringPop(outL, outR) {
      var ring = sysAudio.ring;
      for (var i = 0; i < outL.length; i++) {
        if (ring.count > 0) {
          outL[i] = ring.left[ring.read];
          outR[i] = ring.right[ring.read];
          ring.read = (ring.read + 1) % ring.size;
          ring.count--;
        } else {
          outL[i] = 0;
          outR[i] = 0;
        }
      }
    }

    function startSystemAudio() {
      stopSystemAudio();
      var AC = window.AudioContext || window.webkitAudioContext;
      var context = new AC({ sampleRate: 48000 });
      sysAudio.ring = {
        size: 48000 * 4, // 4 seconds per channel
        left: new Float32Array(48000 * 4),
        right: new Float32Array(48000 * 4),
        read: 0,
        write: 0,
        count: 0
      };
      var processor = context.createScriptProcessor(8192, 0, 2);
      var dest = context.createMediaStreamDestination();
      processor.connect(dest);
      processor.onaudioprocess = function (event) {
        ringPop(event.outputBuffer.getChannelData(0),
                event.outputBuffer.getChannelData(1));
      };
      sysAudio.context = context;
      sysAudio.processor = processor;
      sysAudioActive = true;
      context.resume().catch(function () {});
      invoke('system_audio_start').catch(function (error) {
        pageLog('error', [tag, 'system_audio_start failed:', error.message || error]);
      });
      return dest.stream.getAudioTracks()[0];
    }

    function stopSystemAudio() {
      sysAudioActive = false;
      sysAudio.processor = null;
      sysAudio.ring = null;
      if (sysAudio.context) {
        try { sysAudio.context.close(); } catch (error) {}
        sysAudio.context = null;
      }
      invoke('system_audio_stop').catch(function () {});
    }

    // the bar's speaker toggle decides whether the next share carries our
    // wasapi system audio track. rust owns the state and echoes it back.
    var sysAudioEnabled = false;
    invoke('system_audio_get_enabled').then(function (enabled) {
      sysAudioEnabled = !!enabled;
    }).catch(function () {});
    var sysAudioToggleHandlerId = internals.transformCallback(function (eventData) {
      var payload = eventData && eventData.payload;
      if (!payload) return;
      sysAudioEnabled = !!payload.enabled;
      pageLog('warn', [tag, 'system audio toggle:', sysAudioEnabled ? 'on' : 'off']);
    });
    invoke('plugin:event|listen', {
      event: 'chrome:sys_audio',
      target: { kind: 'Any' },
      handler: sysAudioToggleHandlerId
    }).catch(function (error) {
      pageLog('error', [tag, 'system audio toggle listen failed:', error.message || error]);
    });

    // one pcm listener for the page's lifetime (same pattern as the chrome
    // listeners): the raw listen command resolves with a listener id, not an
    // unlisten fn, so adding and removing one per share leaked listeners and
    // doubled the feed. delivery is gated with a flag instead.
    var sysAudioActive = false;
    var pcmHandlerId = internals.transformCallback(function (eventData) {
      if (!sysAudioActive) return;
      var b64 = eventData && eventData.payload;
      if (typeof b64 !== 'string' || !b64) return;
      try {
        ringPush(b64ToFloat32(b64));
      } catch (error) {
        pageLog('error', [tag, 'system audio chunk failed:', error.message]);
      }
    });
    invoke('plugin:event|listen', {
      event: 'system_audio:pcm',
      target: { kind: 'Any' },
      handler: pcmHandlerId
    }).catch(function (error) {
      pageLog('error', [tag, 'system audio pcm listen failed:', error.message || error]);
    });

    function attachSystemAudio(stream) {
      var videoTrack = stream.getVideoTracks()[0];
      var audioTrack = startSystemAudio();
      videoTrack.addEventListener('ended', function () {
        pageLog('warn', [tag, 'screen share stopped']);
        stopSystemAudio();
      });
      stream.addTrack(audioTrack);
    }

    md.getDisplayMedia = function () {
      var args = Array.prototype.slice.call(arguments);
      var constraints = args[0] || {};
      // strip audio: webview2's loopback source always fails, and leaving the
      // checkbox in would just break the picker. our toggle decides instead.
      var realConstraints = {};
      if (constraints.video) realConstraints.video = constraints.video;
      var promise = orig(realConstraints);
      return Promise.resolve(promise).then(function (stream) {
        if (sysAudioEnabled) {
          attachSystemAudio(stream);
          pageLog('warn', [tag, 'screen share started with system audio']);
        } else {
          pageLog('warn', [tag, 'screen share started']);
        }
        invoke('hide_share_pill').catch(function () {});
        return stream;
      }, function (error) {
        pageLog('warn', [tag, 'getDisplayMedia rejected:',
          error && error.name ? error.name : String(error),
          error && error.message ? error.message : '']);
        throw error;
      });
    };
  })();

  // --- chrome panels ----------------------------------------------------
  var activePanel = null;

  function styleInput(input, placeholder) {
    input.placeholder = placeholder;
    input.maxLength = 64;
    input.style.width = '100%';
    input.style.height = '32px';
    input.style.boxSizing = 'border-box';
    input.style.background = '#1e1f22';
    input.style.border = '1px solid #404249';
    input.style.borderRadius = '4px';
    input.style.color = '#ffffff';
    input.style.padding = '0 8px';
    input.style.marginBottom = '10px';
    input.style.font = '13px "Segoe UI", sans-serif';
  }

  function styleButton(button, label, primary) {
    button.textContent = label;
    button.style.background = primary ? '#5865f2' : '#4e5058';
    button.style.border = 'none';
    button.style.borderRadius = '4px';
    button.style.color = '#ffffff';
    button.style.padding = '7px 14px';
    button.style.cursor = 'pointer';
    button.style.font = '600 12px "Segoe UI", sans-serif';
    if (primary) {
      button.onmouseenter = function () {
        button.style.background = '#4752c4';
      };
      button.onmouseleave = function () {
        button.style.background = '#5865f2';
      };
    }
  }

  // the settings panel edits live in these, so the capture events can update
  // them even though the panel is created and destroyed on demand.
  var liveSettings = null;
  var captureDivs = {};

  function keyFieldFor(field) {
    return field === 'mute' ? 'mute_key' : 'deafen_key';
  }

  function closePanel() {
    if (activePanel && activePanel.parentNode) {
      activePanel.parentNode.removeChild(activePanel);
    }
    activePanel = null;
    liveSettings = null;
    captureDivs = {};
    // if a capture was still armed when the panel went away, drop it.
    invoke('hk_cancel_capture').catch(function () {});
  }

  function createPanel(title, kind) {
    closePanel();
    var panel = document.createElement('div');
    panel.dataset.kind = kind;
    panel.style.position = 'fixed';
    panel.style.top = '36px';
    // the cog that opens these panels sits on the left of the bar, so the
    // panel drops down on the left instead of the right.
    panel.style.left = '10px';
    panel.style.width = '280px';
    panel.style.zIndex = '2147483647';
    panel.style.background = '#2b2d31';
    panel.style.border = '1px solid #1e1f22';
    panel.style.borderRadius = '8px';
    panel.style.padding = '14px';
    panel.style.boxSizing = 'border-box';
    panel.style.color = '#dbdee1';
    panel.style.font = '12px "Segoe UI", sans-serif';
    panel.style.boxShadow = '0 8px 24px rgba(0,0,0,.5)';

    var heading = document.createElement('div');
    heading.textContent = title;
    heading.style.fontWeight = '600';
    heading.style.marginBottom = '12px';
    panel.appendChild(heading);

    activePanel = panel;
    document.body.appendChild(panel);
    return panel;
  }

  function buttonRow(panel) {
    var row = document.createElement('div');
    row.style.textAlign = 'right';
    panel.appendChild(row);
    return row;
  }

  function showAddServerPanel() {
    var panel = createPanel('add favorite server', 'add_server');

    var nameInput = document.createElement('input');
    styleInput(nameInput, 'name');
    panel.appendChild(nameInput);

    var urlInput = document.createElement('input');
    styleInput(urlInput, 'https://server.example');
    panel.appendChild(urlInput);

    var row = buttonRow(panel);
    var cancel = document.createElement('button');
    styleButton(cancel, 'cancel');
    cancel.style.marginRight = '8px';
    cancel.onclick = function () {
      closePanel();
    };
    row.appendChild(cancel);

    var save = document.createElement('button');
    styleButton(save, 'save', true);
    save.onclick = function () {
      var name = nameInput.value.trim();
      var url = urlInput.value.trim();
      if (!name || !url) {
        closePanel();
        return;
      }
      invoke('add_server', { name: name, url: url })
        .then(function () {
          closePanel();
        })
        .catch(function (error) {
          pageLog('error', [tag, 'add_server failed', error]);
          closePanel();
        });
    };
    row.appendChild(save);

    nameInput.focus();
  }

  function startFieldCapture(field, div) {
    div.textContent = 'press a key or mouse button...';
    invoke('hk_start_capture', { which: field }).catch(function (error) {
      pageLog('error', [tag, 'hk_start_capture failed', error]);
      div.textContent = 'failed to arm capture';
    });
  }

  function applyCaptured(field, value) {
    if (liveSettings) {
      liveSettings[keyFieldFor(field)] = value;
    }
    var div = captureDivs[field];
    if (div) div.textContent = value;
  }

  function applyCaptureCancelled(field) {
    var div = captureDivs[field];
    if (!div) return;
    div.textContent =
      (liveSettings && liveSettings[keyFieldFor(field)]) || 'click to bind';
  }

  function showSettingsPanel() {
    var panel = createPanel('keybinds', 'settings');
    liveSettings = {
      mute_key: '',
      mute_mode: 'toggle',
      deafen_key: '',
      deafen_mode: 'toggle'
    };
    captureDivs = {};
    var modeSelects = {};

    var rows = [
      { field: 'mute', label: 'mute', modeField: 'mute_mode' },
      { field: 'deafen', label: 'deafen', modeField: 'deafen_mode' }
    ];

    rows.forEach(function (rowDef) {
      var label = document.createElement('div');
      label.textContent = rowDef.label;
      label.style.color = '#949ba4';
      label.style.fontSize = '12px';
      label.style.marginBottom = '6px';
      panel.appendChild(label);

      var row = document.createElement('div');
      row.style.display = 'flex';
      row.style.marginBottom = '14px';
      panel.appendChild(row);

      // the key field is a clickable capture div, not a text input: clicking
      // it arms the rust hook, which listens for the next key or mouse button
      // press anywhere in the system.
      var capture = document.createElement('div');
      capture.style.flex = '1 1 auto';
      capture.style.minWidth = '0';
      capture.style.background = '#1e1f22';
      capture.style.border = '1px solid #404249';
      capture.style.borderRadius = '4px';
      capture.style.color = '#ffffff';
      capture.style.height = '32px';
      capture.style.display = 'flex';
      capture.style.alignItems = 'center';
      capture.style.justifyContent = 'center';
      capture.style.font = '13px "Segoe UI", sans-serif';
      capture.style.cursor = 'pointer';
      capture.textContent = 'click to bind';
      capture.title = 'click, then press a key or mouse button (esc cancels)';
      capture.addEventListener('mousedown', function (e) {
        e.preventDefault();
        startFieldCapture(rowDef.field, capture);
      });
      row.appendChild(capture);

      var select = document.createElement('select');
      select.style.background = '#1e1f22';
      select.style.border = '1px solid #404249';
      select.style.borderRadius = '4px';
      select.style.color = '#ffffff';
      select.style.height = '32px';
      select.style.width = '86px';
      select.style.marginLeft = '8px';
      select.style.font = '13px "Segoe UI", sans-serif';
      var optToggle = document.createElement('option');
      optToggle.value = 'toggle';
      optToggle.textContent = 'toggle';
      var optHold = document.createElement('option');
      optHold.value = 'hold';
      optHold.textContent = 'hold';
      select.appendChild(optToggle);
      select.appendChild(optHold);
      row.appendChild(select);

      captureDivs[rowDef.field] = capture;
      modeSelects[rowDef.modeField] = select;
    });

    var row = buttonRow(panel);
    var close = document.createElement('button');
    styleButton(close, 'save and close', true);
    close.onclick = function () {
      invoke('save_settings', { settings: liveSettings })
        .then(function () {
          closePanel();
        })
        .catch(function (error) {
          pageLog('error', [tag, 'save_settings failed', error]);
          closePanel();
        });
    };
    row.appendChild(close);

    invoke('get_settings')
      .then(function (settings) {
        liveSettings.mute_key = settings.mute_key || '';
        liveSettings.mute_mode = settings.mute_mode || 'toggle';
        liveSettings.deafen_key = settings.deafen_key || '';
        liveSettings.deafen_mode = settings.deafen_mode || 'toggle';
        captureDivs.mute.textContent = liveSettings.mute_key || 'click to bind';
        captureDivs.deafen.textContent = liveSettings.deafen_key || 'click to bind';
        modeSelects.mute_mode.value = liveSettings.mute_mode;
        modeSelects.deafen_mode.value = liveSettings.deafen_mode;
      })
      .catch(function () {});
  }

  // --- keybinds (mute and deafen) ---------------------------------------
  // the hosted client renders a mute button (title contains "microphone") and
  // a deafen button (title contains "deafen") in the user control. clicking a
  // button runs the client's own toggle (local state + tRPC), so no access to
  // the store or tRPC is needed. the title is stable across states, and the
  // "unmute"/"undeafen" prefix marks the active state.
  //
  // the keys themselves are matched by the rust global input hook, which
  // emits chrome:hotkey events; this side only clicks the buttons.
  var voiceActions = {
    mute: { keyword: 'microphone', activeKeyword: 'unmute' },
    deafen: { keyword: 'deafen', activeKeyword: 'undeafen' }
  };

  function findVoiceButton(keyword) {
    var buttons = document.querySelectorAll('button');
    for (var i = 0; i < buttons.length; i++) {
      var title = (buttons[i].getAttribute('title') || '').toLowerCase();
      if (title.indexOf(keyword) !== -1) return buttons[i];
    }
    return null;
  }

  // set the state to active or inactive, clicking only when it differs.
  // a disabled button is left alone (deafen disables the mute button).
  function setVoiceState(action, active) {
    var button = findVoiceButton(action.keyword);
    if (!button || button.disabled) return;
    var title = (button.getAttribute('title') || '').toLowerCase();
    var current = title.indexOf(action.activeKeyword) !== -1;
    if (current === active) return;
    button.click();
  }

  function toggleVoiceState(action) {
    var button = findVoiceButton(action.keyword);
    if (!button || button.disabled) return;
    button.click();
  }

  // ignore hotkeys while a chrome panel is open or the page has an editable
  // field focused, so the settings inputs do not trigger the very keys they
  // configure.
  function shouldSuppressHotkey() {
    if (activePanel !== null) return true;
    var el = document.activeElement;
    if (!el || !el.tagName) return false;
    var name = el.tagName.toLowerCase();
    if (name === 'input' || name === 'textarea' || name === 'select') return true;
    if (el.isContentEditable) return true;
    return false;
  }

  var hotkeyHandlerId = internals.transformCallback(function (eventData) {
    var payload = eventData && eventData.payload;
    if (!payload) return;
    if (shouldSuppressHotkey()) return;
    var action = voiceActions[payload.action];
    if (!action) return;
    if (payload.cmd === 'toggle') {
      toggleVoiceState(action);
    } else if (payload.cmd === 'hold_down') {
      setVoiceState(action, true);
    } else if (payload.cmd === 'hold_up') {
      setVoiceState(action, false);
    }
  });
  invoke('plugin:event|listen', {
    event: 'chrome:hotkey',
    target: { kind: 'Any' },
    handler: hotkeyHandlerId
  }).catch(function (error) {
    pageLog('error', [tag, 'chrome hotkey listen failed', error]);
  });

  var capturedHandlerId = internals.transformCallback(function (eventData) {
    var payload = eventData && eventData.payload;
    if (!payload || !payload.field) return;
    if (payload.field === 'mute' || payload.field === 'deafen') {
      applyCaptured(payload.field, payload.value);
    }
  });
  invoke('plugin:event|listen', {
    event: 'chrome:captured',
    target: { kind: 'Any' },
    handler: capturedHandlerId
  }).catch(function (error) {
    pageLog('error', [tag, 'chrome captured listen failed', error]);
  });

  var cancelledHandlerId = internals.transformCallback(function (eventData) {
    var payload = eventData && eventData.payload;
    if (!payload || !payload.field) return;
    if (payload.field === 'mute' || payload.field === 'deafen') {
      applyCaptureCancelled(payload.field);
    }
  });
  invoke('plugin:event|listen', {
    event: 'chrome:capture_cancelled',
    target: { kind: 'Any' },
    handler: cancelledHandlerId
  }).catch(function (error) {
    pageLog('error', [tag, 'chrome capture_cancelled listen failed', error]);
  });

  // the bar webview emits these events to open a panel in the page.
  var chromeHandlerId = internals.transformCallback(function (eventData) {
    var event = eventData && eventData.event;
    if (event === 'chrome:show_add_server') {
      showAddServerPanel();
    } else if (event === 'chrome:show_settings') {
      // the cog toggles: if the keybinds panel is already open, close it.
      if (activePanel && activePanel.dataset.kind === 'settings') {
        closePanel();
      } else {
        showSettingsPanel();
      }
    }
  });
  invoke('plugin:event|listen', {
    event: 'chrome:show_add_server',
    target: { kind: 'Any' },
    handler: chromeHandlerId
  }).catch(function (error) {
    pageLog('error', [tag, 'chrome add_server listen failed', error]);
  });
  invoke('plugin:event|listen', {
    event: 'chrome:show_settings',
    target: { kind: 'Any' },
    handler: chromeHandlerId
  }).catch(function (error) {
    pageLog('error', [tag, 'chrome settings listen failed', error]);
  });
})();
