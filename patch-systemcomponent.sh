#!/usr/bin/env bash
# Patch stock JMP files for VLC compatibility
# Usage: ./patch-systemcomponent.sh <path-to-jmp-src>
set -euo pipefail
JMP_SRC="$1"
HEADER="$JMP_SRC/src/system/SystemComponent.h"
CPP="$JMP_SRC/src/system/SystemComponent.cpp"

# Add getEnvironmentVariable declaration to header
if ! grep -q getEnvironmentVariable "$HEADER"; then
    python3 -c "
import sys; f=sys.argv[1]; c=open(f).read()
m='Q_INVOKABLE void openExternalUrl(const QString& url);'
c=c.replace(m, m+'\n\n  Q_INVOKABLE QString getEnvironmentVariable(const QString& name);')
open(f,'w').write(c)
" "$HEADER"
    echo "  Patched header"
else
    echo "  Header already patched"
fi

# Add getEnvironmentVariable implementation to cpp
if ! grep -q getEnvironmentVariable "$CPP"; then
    cat >> "$CPP" << 'CPPEOF'

///////////////////////////////////////////////////////////////////////////////////////////////////
QString SystemComponent::getEnvironmentVariable(const QString& name)
{
  return QString::fromUtf8(qgetenv(name.toUtf8().constData()));
}
CPPEOF
    echo "  Patched cpp"
else
    echo "  Cpp already patched"
fi

# Patch KonvergoWindow.cpp to guard PlayerQuickItem references
KONVERGO="$JMP_SRC/src/ui/KonvergoWindow.cpp"
if grep -q "PlayerQuickItem" "$KONVERGO"; then
    python3 -c "
import sys; f=sys.argv[1]; c=open(f).read()
c=c.replace('#include \"player/PlayerQuickItem.h\"', '#ifndef USE_VLC\n#include \"player/PlayerQuickItem.h\"\n#endif')
old='  PlayerQuickItem* video = findChild<PlayerQuickItem*>(\"video\");\n  if (video)\n    m_debugInfo += video->debugInfo();'
c=c.replace(old, '#ifndef USE_VLC\n'+old+'\n#endif')
open(f,'w').write(c)
" "$KONVERGO"
    echo "  Patched KonvergoWindow.cpp"
fi

# --- Patch getNativeShellScript to prepend qwebchannel.js and inline plugin JS files ---
if ! grep -q "Inlining plugin" "$CPP"; then
    # Write python patch script to temp file to avoid shell quoting issues
    PYSCRIPT=$(mktemp /tmp/patch_nss_XXXXXX.py)
    python3 -c "
# Generate the python patch script
script = '''import sys

f = sys.argv[1]
c = open(f).read()
Q = chr(34)
NL = chr(10)
BS = chr(92)

old = '  nativeshellString.replace(' + Q + '@@data@@' + Q + ', QJsonDocument(clientData).toJson(QJsonDocument::Compact).toBase64());' + NL + '  return nativeshellString;'

new = '  nativeshellString.replace(' + Q + '@@data@@' + Q + ', QJsonDocument(clientData).toJson(QJsonDocument::Compact).toBase64());' + NL
new += NL
new += '  // Prepend qwebchannel.js from Qt resources so QWebChannel JS class is available' + NL
new += '  // This is needed when loading remote web client (HTTPS) where Qt cannot auto-inject it' + NL
new += '  QFile qwcFile(' + Q + ':/qtwebchannel/qwebchannel.js' + Q + ');' + NL
new += '  if (qwcFile.open(QIODevice::ReadOnly)) {' + NL
new += '    QString qwcScript = QTextStream(&qwcFile).readAll();' + NL
new += '    nativeshellString = qwcScript + ' + Q + BS + 'n' + Q + ' + nativeshellString;' + NL
new += '    qDebug() << ' + Q + 'Prepended qwebchannel.js from Qt resources (' + Q + ' << qwcScript.size() << ' + Q + ' bytes)' + Q + ';' + NL
new += '  } else {' + NL
new += '    qWarning() << ' + Q + 'FAILED to load qwebchannel.js from Qt resources - QWebChannel will not work!' + Q + ';' + NL
new += '  }' + NL
new += NL
new += '  // Inlining plugin JS files to avoid CORS issues with remote web client' + NL
new += '  QStringList pluginFiles = {' + Q + 'mpvVideoPlayer.js' + Q + ', ' + Q + 'mpvAudioPlayer.js' + Q + ', ' + Q + 'jmpInputPlugin.js' + Q + ', ' + Q + 'jmpUpdatePlugin.js' + Q + ', ' + Q + 'skipIntroPlugin.js' + Q + '};' + NL
new += '  QString inlinedPlugins;' + NL
new += '  for (const auto& pluginFile : pluginFiles) {' + NL
new += '    QFile pf(path + pluginFile);' + NL
new += '    if (pf.open(QIODevice::ReadOnly)) {' + NL
new += '      qDebug() << ' + Q + 'Inlining plugin:' + Q + ' << pluginFile;' + NL
new += '      inlinedPlugins += ' + Q + BS + 'n// === Inlined: ' + Q + ' + pluginFile + ' + Q + ' ===' + BS + 'n' + Q + ';' + NL
new += '      inlinedPlugins += QTextStream(&pf).readAll();' + NL
new += '      inlinedPlugins += ' + Q + BS + 'n' + Q + ';' + NL
new += '    } else {' + NL
new += '      qWarning() << ' + Q + 'Could not inline plugin:' + Q + ' << pluginFile;' + NL
new += '    }' + NL
new += '  }' + NL
new += '  nativeshellString += inlinedPlugins;' + NL
new += NL
new += '  return nativeshellString;'

if old not in c:
    print('WARNING: Target pattern not found in getNativeShellScript - may be already patched', file=sys.stderr)
    sys.exit(0)

c = c.replace(old, new)
open(f, 'w').write(c)
print('  Patched getNativeShellScript for qwebchannel.js + plugin inlining')
'''
import sys
open(sys.argv[1], 'w').write(script)
" "$PYSCRIPT"
    python3 "$PYSCRIPT" "$CPP"
    rm -f "$PYSCRIPT"
fi
