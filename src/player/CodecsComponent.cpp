//
// CodecsComponent.cpp - VLC backend stub
//
// The stock CodecsComponent.cpp is tightly coupled to mpv/libav for codec
// detection and downloading. Since we use VLC (which bundles its own codecs),
// this file provides minimal stub implementations of all required symbols.
//

#include "CodecsComponent.h"
#include <QDebug>
#include <QDir>
#include <shared/Paths.h>

///////////////////////////////////////////////////////////////////////////////////////////////////
// CodecDriver methods
///////////////////////////////////////////////////////////////////////////////////////////////////

QString CodecDriver::getMangledName() const
{
  return driver + (type == CodecType::Decoder ? "_decoder" : "_encoder");
}

QString CodecDriver::getFileName() const
{
  return QString();
}

QString CodecDriver::getPath() const
{
  return QString();
}

bool CodecDriver::isSystemCodec() const
{
  return false;
}

QString CodecDriver::getSystemCodecType() const
{
  return QString();
}

bool CodecDriver::isWhitelistedSystemAudioCodec() const
{
  return false;
}

bool CodecDriver::isWhitelistedSystemVideoCodec() const
{
  return false;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Codecs static methods
///////////////////////////////////////////////////////////////////////////////////////////////////

static QList<CodecDriver> g_cachedCodecList;

void Codecs::preinitCodecs()
{
  qInfo() << "Codecs::preinitCodecs - VLC backend, no codec preinitialization needed";
}

void Codecs::initCodecs()
{
  qInfo() << "Codecs::initCodecs - VLC backend, codecs managed by VLC";
}

QString Codecs::plexNameToFF(QString plex)
{
  // Basic mapping for common codecs
  if (plex == "dca") return "dts";
  return plex;
}

QString Codecs::plexNameFromFF(QString ffname)
{
  if (ffname == "dts") return "dca";
  return ffname;
}

void Codecs::updateCachedCodecList()
{
  // VLC manages its own codecs - nothing to cache
  g_cachedCodecList.clear();
}

void Codecs::Uninit()
{
  g_cachedCodecList.clear();
}

const QList<CodecDriver>& Codecs::getCachedCodecList()
{
  return g_cachedCodecList;
}

QList<CodecDriver> Codecs::findCodecsByFormat(const QList<CodecDriver>& list, CodecType type, const QString& format)
{
  QList<CodecDriver> result;
  for (const CodecDriver& d : list) {
    if (d.type == type && d.format == format)
      result.append(d);
  }
  return result;
}

QList<CodecDriver> Codecs::determineRequiredCodecs(const PlaybackInfo& info)
{
  Q_UNUSED(info);
  // VLC handles all codecs internally - no external codecs needed
  return QList<CodecDriver>();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Downloader
///////////////////////////////////////////////////////////////////////////////////////////////////

Downloader::Downloader(QVariant userData, const QUrl& url, const HeaderList& headers, QObject* parent)
  : QObject(parent), m_userData(userData), m_lastProgress(0)
{
  Q_UNUSED(url);
  Q_UNUSED(headers);
}

void Downloader::networkFinished(QNetworkReply* pReply)
{
  Q_UNUSED(pReply);
}

void Downloader::downloadProgress(qint64 bytesReceived, qint64 bytesTotal)
{
  Q_UNUSED(bytesReceived);
  Q_UNUSED(bytesTotal);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// CodecsFetcher
///////////////////////////////////////////////////////////////////////////////////////////////////

void CodecsFetcher::installCodecs(const QList<CodecDriver>& codecs)
{
  Q_UNUSED(codecs);
  // VLC manages its own codecs - nothing to install
  emit done(this);
}

bool CodecsFetcher::codecNeedsDownload(const CodecDriver& codec)
{
  Q_UNUSED(codec);
  return false;
}

bool CodecsFetcher::processCodecInfoReply(const QVariant& context, const QByteArray& data)
{
  Q_UNUSED(context);
  Q_UNUSED(data);
  return false;
}

void CodecsFetcher::processCodecDownloadDone(const QVariant& context, const QByteArray& data)
{
  Q_UNUSED(context);
  Q_UNUSED(data);
}

void CodecsFetcher::startNext()
{
  // Nothing to download
  emit done(this);
}

void CodecsFetcher::startEAE()
{
  // EAE not needed for VLC
}

void CodecsFetcher::codecInfoDownloadDone(QVariant userData, bool success, const QByteArray& data)
{
  Q_UNUSED(userData);
  Q_UNUSED(success);
  Q_UNUSED(data);
}

void CodecsFetcher::codecDownloadDone(QVariant userData, bool success, const QByteArray& data)
{
  Q_UNUSED(userData);
  Q_UNUSED(success);
  Q_UNUSED(data);
}
