#include <QtGlobal>
#include <QSurfaceFormat>
#include <QCoreApplication>
#include <QOpenGLContext>
#include <QDebug>

#ifdef USE_VLC
// VLC mode: no mpv-based GL probing needed
#else
#include <mpv/client.h>
#include "QtHelper.h"
#endif

#include "OpenGLDetect.h"

#if defined(Q_OS_MAC)

void detectOpenGLEarly()
{
  QSurfaceFormat format = QSurfaceFormat::defaultFormat();
  format.setMajorVersion(3);
  format.setMinorVersion(2);
  format.setProfile(QSurfaceFormat::CoreProfile);
  QSurfaceFormat::setDefaultFormat(format);
}

void detectOpenGLLate()
{
}

#elif defined(Q_OS_LINUX) || defined(Q_OS_FREEBSD)

#ifdef USE_VLC

// VLC mode: skip mpv-based hwdec-interop probing.
// Default to xcb_egl on Pi 5 for best hardware decode compatibility.
void detectOpenGLEarly()
{
  qputenv("QT_XCB_GL_INTEGRATION", "xcb_egl");
}

void detectOpenGLLate()
{
}

#else

static QString probeHwdecInterop()
{
  auto mpv = mpv::qt::Handle::FromRawHandle(mpv_create());
  if (!mpv)
    return "";
  mpv::qt::set_property(mpv, "force-window", true);
  mpv::qt::set_property(mpv, "geometry", "1x1+0+0");
  mpv::qt::set_property(mpv, "border", false);
  if (mpv_initialize(mpv) < 0)
    return "";
  return mpv::qt::get_property(mpv, "hwdec-interop").toString();
}

void detectOpenGLEarly()
{
  if (probeHwdecInterop() == "vaapi-egl")
    qputenv("QT_XCB_GL_INTEGRATION", "xcb_egl");
}

void detectOpenGLLate()
{
}

#endif // USE_VLC

#elif defined(Q_OS_WIN)

void detectOpenGLEarly()
{
}

void detectOpenGLLate()
{
  if (!QCoreApplication::testAttribute(Qt::AA_UseOpenGLES))
    return;
  qputenv("QML_USE_GLYPHCACHE_WORKAROUND", "1");
  QList<int> versions = { 3, 2 };
  for (auto version : versions)
  {
    QSurfaceFormat fmt = QSurfaceFormat::defaultFormat();
    fmt.setMajorVersion(version);
    QOpenGLContext ctx;
    ctx.setFormat(fmt);
    if (ctx.create())
    {
      QSurfaceFormat::setDefaultFormat(fmt);
      break;
    }
  }
}

#else

void detectOpenGLEarly()
{
}

void detectOpenGLLate()
{
}

#endif
