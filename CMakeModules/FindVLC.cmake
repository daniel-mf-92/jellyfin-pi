###############################################################################
# CMake module to search for the VLC (libvlc) libraries.
#
# Sets:
#   VLC_FOUND         - TRUE if libvlc was found
#   VLC_INCLUDE_DIRS  - Path to vlc/vlc.h
#   VLC_LIBRARIES     - Libraries to link against
#   VLC_VERSION       - Version string from pkg-config (if available)
#
###############################################################################

SET(_VLC_REQUIRED_VARS VLC_INCLUDE_DIR VLC_LIBRARY)

#
### VLC uses pkgconfig.
#
find_package(PkgConfig QUIET)
if(PKG_CONFIG_FOUND)
  pkg_check_modules(PC_VLC QUIET libvlc)
endif()

#
### Look for the include files.
#
find_path(
  VLC_INCLUDE_DIR
  NAMES vlc/vlc.h
  HINTS
    ${PC_VLC_INCLUDEDIR}
    ${PC_VLC_INCLUDE_DIRS}
  PATHS
    /usr/include
    /usr/local/include
  DOC "VLC include directory"
)

#
### Look for the libraries.
#
find_library(
  VLC_LIBRARY
  NAMES vlc
  HINTS
    ${PC_VLC_LIBDIR}
    ${PC_VLC_LIBRARY_DIRS}
  PATHS
    /usr/lib
    /usr/lib/aarch64-linux-gnu
    /usr/lib/x86_64-linux-gnu
    /usr/local/lib
  PATH_SUFFIXES lib${LIB_SUFFIX}
  DOC "VLC library"
)

find_library(
  VLCCORE_LIBRARY
  NAMES vlccore
  HINTS
    ${PC_VLC_LIBDIR}
    ${PC_VLC_LIBRARY_DIRS}
  PATHS
    /usr/lib
    /usr/lib/aarch64-linux-gnu
    /usr/lib/x86_64-linux-gnu
    /usr/local/lib
  PATH_SUFFIXES lib${LIB_SUFFIX}
  DOC "VLC core library"
)

mark_as_advanced(VLC_INCLUDE_DIR VLC_LIBRARY VLCCORE_LIBRARY)

set(VLC_INCLUDE_DIRS ${VLC_INCLUDE_DIR})
set(VLC_LIBRARIES ${VLC_LIBRARY})
if(VLCCORE_LIBRARY)
  list(APPEND VLC_LIBRARIES ${VLCCORE_LIBRARY})
endif()

if(PC_VLC_VERSION)
  set(VLC_VERSION ${PC_VLC_VERSION})
endif()

#
### Check if everything was found.
#
include(FindPackageHandleStandardArgs)
find_package_handle_standard_args(
  VLC
  REQUIRED_VARS ${_VLC_REQUIRED_VARS}
  VERSION_VAR VLC_VERSION
)
