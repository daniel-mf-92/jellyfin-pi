#include "StreamCacheManager.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QDateTime>
#include <QNetworkAccessManager>
#include <QNetworkRequest>
#include <QNetworkReply>
#include <QUrl>

static const char* CACHE_DIR = "/dev/shm/jmp-cache";

///////////////////////////////////////////////////////////////////////////////////////////////////
StreamCacheManager::StreamCacheManager(QObject* parent)
  : QObject(parent)
  , m_cacheDir(QString::fromLatin1(CACHE_DIR))
{
  QDir dir(m_cacheDir);
  if (!dir.exists())
    dir.mkpath(".");

  // Scan existing files from previous session
  for (const QFileInfo& fi : dir.entryInfoList(QDir::Files))
  {
    CacheEntry e;
    e.path = fi.absoluteFilePath();
    e.size = fi.size();
    e.complete = true;
    e.downloading = false;
    e.lastAccess = fi.lastModified().toSecsSinceEpoch();
    m_entries.insert(fi.baseName(), e);
  }

  if (!m_entries.isEmpty())
    qInfo() << "StreamCacheManager: recovered" << m_entries.size() << "cached items from previous session";

  // Pressure monitor — every 5 seconds
  m_pressureTimer = new QTimer(this);
  m_pressureTimer->setInterval(5000);
  connect(m_pressureTimer, &QTimer::timeout, this, &StreamCacheManager::pressureCheck);
  m_pressureTimer->start();

  qInfo() << "StreamCacheManager: dir=" << m_cacheDir << "floor=" << PRESSURE_FLOOR_GB << "GB";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
StreamCacheManager::~StreamCacheManager()
{
  m_pressureTimer->stop();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QString StreamCacheManager::getCached(const QString& itemId)
{
  QMutexLocker lock(&m_mutex);
  auto it = m_entries.find(itemId);
  if (it != m_entries.end() && it->complete && QFile::exists(it->path))
  {
    it->lastAccess = QDateTime::currentSecsSinceEpoch();
    return it->path;
  }
  return QString();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void StreamCacheManager::startDownload(const QString& itemId, const QString& url)
{
  QMutexLocker lock(&m_mutex);
  auto it = m_entries.find(itemId);
  if (it != m_entries.end() && (it->complete || it->downloading))
    return;  // already done or in progress

  CacheEntry e;
  e.path = m_cacheDir + "/" + itemId;
  e.size = 0;
  e.complete = false;
  e.downloading = true;
  e.lastAccess = QDateTime::currentSecsSinceEpoch();
  m_entries.insert(itemId, e);
  lock.unlock();

  // Download in a background thread
  QThread* thread = QThread::create([this, itemId, url]() {
    doDownload(itemId, url);
  });
  thread->start();
  // Thread self-deletes when finished
  connect(thread, &QThread::finished, thread, &QThread::deleteLater);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void StreamCacheManager::cancelDownload(const QString& itemId)
{
  QMutexLocker lock(&m_mutex);
  auto it = m_entries.find(itemId);
  if (it != m_entries.end())
    it->downloading = false;  // download loop checks this flag
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void StreamCacheManager::pin(const QString& itemId)
{
  QMutexLocker lock(&m_mutex);
  m_pinnedId = itemId;
  auto it = m_entries.find(itemId);
  if (it != m_entries.end())
    it->lastAccess = QDateTime::currentSecsSinceEpoch();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void StreamCacheManager::unpin()
{
  QMutexLocker lock(&m_mutex);
  if (!m_pinnedId.isEmpty())
  {
    auto it = m_entries.find(m_pinnedId);
    if (it != m_entries.end())
      it->lastAccess = QDateTime::currentSecsSinceEpoch();
  }
  m_pinnedId.clear();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
QString StreamCacheManager::stats() const
{
  QMutexLocker lock(&const_cast<StreamCacheManager*>(this)->m_mutex);
  qint64 total = 0;
  for (const auto& e : m_entries)
    total += e.size;
  return QString("%1 items %2MB pinned=%3")
    .arg(m_entries.size())
    .arg(total / (1024 * 1024))
    .arg(m_pinnedId.isEmpty() ? "none" : m_pinnedId.left(8));
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void StreamCacheManager::pressureCheck()
{
  qint64 avail = readMemAvailable();
  qint64 floor = static_cast<qint64>(PRESSURE_FLOOR_GB * 1024.0 * 1024 * 1024);
  if (avail > 0 && avail < floor)
    evictLru();
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void StreamCacheManager::evictLru()
{
  QMutexLocker lock(&m_mutex);

  QString victimId;
  double oldestAccess = std::numeric_limits<double>::max();

  for (auto it = m_entries.begin(); it != m_entries.end(); ++it)
  {
    if (it.key() == m_pinnedId)
      continue;
    if (it->downloading)
      continue;
    if (it->lastAccess < oldestAccess)
    {
      oldestAccess = it->lastAccess;
      victimId = it.key();
    }
  }

  if (victimId.isEmpty())
  {
    qWarning() << "StreamCacheManager: pressure but nothing evictable";
    return;
  }

  CacheEntry victim = m_entries.value(victimId);
  QFile::remove(victim.path);
  m_entries.remove(victimId);

  qInfo() << "StreamCacheManager: evicted" << victimId.left(8)
           << "freed=" << (victim.size / (1024 * 1024)) << "MB"
           << "[" << stats() << "]";
}

///////////////////////////////////////////////////////////////////////////////////////////////////
qint64 StreamCacheManager::readMemAvailable() const
{
  QFile meminfo("/proc/meminfo");
  if (!meminfo.open(QIODevice::ReadOnly | QIODevice::Text))
    return 0;

  while (!meminfo.atEnd())
  {
    QByteArray line = meminfo.readLine();
    if (line.startsWith("MemAvailable:"))
    {
      QList<QByteArray> parts = line.simplified().split(' ');
      if (parts.size() >= 2)
        return parts[1].toLongLong() * 1024;
    }
  }
  return 0;
}

///////////////////////////////////////////////////////////////////////////////////////////////////
void StreamCacheManager::doDownload(const QString& itemId, const QString& url)
{
  QMutexLocker lock(&m_mutex);
  auto it = m_entries.find(itemId);
  if (it == m_entries.end())
    return;
  QString path = it->path;
  lock.unlock();

  qInfo() << "StreamCacheManager: downloading" << itemId.left(8) << "...";

  QFile file(path);
  if (!file.open(QIODevice::WriteOnly))
  {
    qWarning() << "StreamCacheManager: cannot open" << path << "for writing";
    QMutexLocker lock2(&m_mutex);
    if (m_entries.contains(itemId))
      m_entries[itemId].downloading = false;
    return;
  }

  // Use QNetworkAccessManager for the download
  QNetworkAccessManager nam;
  QNetworkRequest req(QUrl(url));
  req.setAttribute(QNetworkRequest::RedirectPolicyAttribute,
                    QNetworkRequest::NoLessSafeRedirectPolicy);

  QNetworkReply* reply = nam.get(req);

  // Block until finished (we're in a background thread)
  QEventLoop loop;
  QObject::connect(reply, &QNetworkReply::finished, &loop, &QEventLoop::quit);

  // Read data as it arrives
  QObject::connect(reply, &QNetworkReply::readyRead, [&]() {
    QByteArray chunk = reply->readAll();
    file.write(chunk);

    QMutexLocker lock2(&m_mutex);
    auto it2 = m_entries.find(itemId);
    if (it2 != m_entries.end())
    {
      it2->size += chunk.size();
      // Check if download was canceled
      if (!it2->downloading)
      {
        reply->abort();
        return;
      }
    }

    // Pause under memory pressure
    qint64 avail = readMemAvailable();
    qint64 floor = static_cast<qint64>(PRESSURE_FLOOR_GB * 1024.0 * 1024 * 1024);
    if (avail > 0 && avail < floor)
    {
      qInfo() << "StreamCacheManager: download paused (" << itemId.left(8) << "): RAM pressure";
      QThread::sleep(5);
    }
  });

  loop.exec();

  file.close();
  reply->deleteLater();

  QMutexLocker lock3(&m_mutex);
  auto it3 = m_entries.find(itemId);
  if (it3 != m_entries.end())
  {
    if (reply->error() == QNetworkReply::NoError)
    {
      it3->complete = true;
      it3->downloading = false;
      qInfo() << "StreamCacheManager: complete" << itemId.left(8)
              << "size=" << (it3->size / (1024 * 1024)) << "MB"
              << "[" << stats() << "]";
    }
    else
    {
      it3->downloading = false;
      qWarning() << "StreamCacheManager: download failed" << itemId.left(8)
                 << reply->errorString();
    }
  }
}
