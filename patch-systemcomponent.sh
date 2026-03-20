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
