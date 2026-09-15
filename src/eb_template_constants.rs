/// EasyBuild TEMPLATE_CONSTANTS (mirrors easybuild.framework.easyconfig.templates).
/// Values keep %(…)s placeholders; applied when name/version are known.
pub const EB_TEMPLATE_CONSTANTS: &[(&str, &str)] = &[
    (
        "APACHE_SOURCE",
        "https://archive.apache.org/dist/%(namelower)s",
    ),
    (
        "BITBUCKET_DOWNLOADS",
        "https://bitbucket.org/%(bitbucket_account)s/%(namelower)s/downloads",
    ),
    (
        "BITBUCKET_SOURCE",
        "https://bitbucket.org/%(bitbucket_account)s/%(namelower)s/get",
    ),
    ("CRAN_SOURCE", "https://cran.r-project.org/src/contrib"),
    (
        "FTPGNOME_SOURCE",
        "https://ftp.gnome.org/pub/GNOME/sources/%(namelower)s/%(version_major_minor)s",
    ),
    (
        "GITHUB_LOWER_RELEASE",
        "https://github.com/%(github_account)s/%(namelower)s/releases/download/v%(version)s",
    ),
    (
        "GITHUB_LOWER_SOURCE",
        "https://github.com/%(github_account)s/%(namelower)s/archive",
    ),
    (
        "GITHUB_RELEASE",
        "https://github.com/%(github_account)s/%(name)s/releases/download/v%(version)s",
    ),
    (
        "GITHUB_SOURCE",
        "https://github.com/%(github_account)s/%(name)s/archive",
    ),
    ("GNU_FTP_SOURCE", "https://ftp.gnu.org/gnu/%(namelower)s"),
    (
        "GNU_SAVANNAH_SOURCE",
        "https://download-mirror.savannah.gnu.org/releases/%(namelower)s",
    ),
    ("GNU_SOURCE", "https://ftpmirror.gnu.org/gnu/%(namelower)s"),
    (
        "GOOGLECODE_SOURCE",
        "http://%(namelower)s.googlecode.com/files",
    ),
    (
        "LAUNCHPAD_SOURCE",
        "https://launchpad.net/%(namelower)s/%(version_major_minor)s.x/%(version)s/+download/",
    ),
    (
        "PYPI_LOWER_SOURCE",
        "https://pypi.python.org/packages/source/%(nameletterlower)s/%(namelower)s",
    ),
    (
        "PYPI_SOURCE",
        "https://pypi.python.org/packages/source/%(nameletter)s/%(name)s",
    ),
    (
        "R_SOURCE",
        "https://cran.r-project.org/src/base/R-%(version_major)s",
    ),
    ("SHLIB_EXT", SHLIB_EXT),
    (
        "SOURCEFORGE_SOURCE",
        "https://download.sourceforge.net/%(namelower)s",
    ),
    ("SOURCELOWER_GTGZ", "%(namelower)s-%(version)s.gtgz"),
    (
        "SOURCELOWER_PY2_WHL",
        "%(namelower)s-%(version)s-py2-none-any.whl",
    ),
    (
        "SOURCELOWER_PY3_WHL",
        "%(namelower)s-%(version)s-py3-none-any.whl",
    ),
    ("SOURCELOWER_TAR", "%(namelower)s-%(version)s.tar"),
    ("SOURCELOWER_TAR_BZ2", "%(namelower)s-%(version)s.tar.bz2"),
    ("SOURCELOWER_TAR_GZ", "%(namelower)s-%(version)s.tar.gz"),
    ("SOURCELOWER_TAR_XZ", "%(namelower)s-%(version)s.tar.xz"),
    ("SOURCELOWER_TAR_Z", "%(namelower)s-%(version)s.tar.Z"),
    ("SOURCELOWER_TB2", "%(namelower)s-%(version)s.tb2"),
    ("SOURCELOWER_TBZ2", "%(namelower)s-%(version)s.tbz2"),
    ("SOURCELOWER_TGZ", "%(namelower)s-%(version)s.tgz"),
    ("SOURCELOWER_TXZ", "%(namelower)s-%(version)s.txz"),
    (
        "SOURCELOWER_WHL",
        "%(namelower)s-%(version)s-py2.py3-none-any.whl",
    ),
    ("SOURCELOWER_XZ", "%(namelower)s-%(version)s.xz"),
    ("SOURCELOWER_ZIP", "%(namelower)s-%(version)s.zip"),
    ("SOURCE_GTGZ", "%(name)s-%(version)s.gtgz"),
    ("SOURCE_PY2_WHL", "%(name)s-%(version)s-py2-none-any.whl"),
    ("SOURCE_PY3_WHL", "%(name)s-%(version)s-py3-none-any.whl"),
    ("SOURCE_TAR", "%(name)s-%(version)s.tar"),
    ("SOURCE_TAR_BZ2", "%(name)s-%(version)s.tar.bz2"),
    ("SOURCE_TAR_GZ", "%(name)s-%(version)s.tar.gz"),
    ("SOURCE_TAR_XZ", "%(name)s-%(version)s.tar.xz"),
    ("SOURCE_TAR_Z", "%(name)s-%(version)s.tar.Z"),
    ("SOURCE_TB2", "%(name)s-%(version)s.tb2"),
    ("SOURCE_TBZ2", "%(name)s-%(version)s.tbz2"),
    ("SOURCE_TGZ", "%(name)s-%(version)s.tgz"),
    ("SOURCE_TXZ", "%(name)s-%(version)s.txz"),
    ("SOURCE_WHL", "%(name)s-%(version)s-py2.py3-none-any.whl"),
    ("SOURCE_XZ", "%(name)s-%(version)s.xz"),
    ("SOURCE_ZIP", "%(name)s-%(version)s.zip"),
    ("VERSION_GTGZ", "%(version)s.gtgz"),
    ("VERSION_TAR", "%(version)s.tar"),
    ("VERSION_TAR_BZ2", "%(version)s.tar.bz2"),
    ("VERSION_TAR_GZ", "%(version)s.tar.gz"),
    ("VERSION_TAR_XZ", "%(version)s.tar.xz"),
    ("VERSION_TAR_Z", "%(version)s.tar.Z"),
    ("VERSION_TB2", "%(version)s.tb2"),
    ("VERSION_TBZ2", "%(version)s.tbz2"),
    ("VERSION_TGZ", "%(version)s.tgz"),
    ("VERSION_TXZ", "%(version)s.txz"),
    ("VERSION_XZ", "%(version)s.xz"),
    ("VERSION_ZIP", "%(version)s.zip"),
    ("V_VERSION_GTGZ", "v%(version)s.gtgz"),
    ("V_VERSION_TAR", "v%(version)s.tar"),
    ("V_VERSION_TAR_BZ2", "v%(version)s.tar.bz2"),
    ("V_VERSION_TAR_GZ", "v%(version)s.tar.gz"),
    ("V_VERSION_TAR_XZ", "v%(version)s.tar.xz"),
    ("V_VERSION_TAR_Z", "v%(version)s.tar.Z"),
    ("V_VERSION_TB2", "v%(version)s.tb2"),
    ("V_VERSION_TBZ2", "v%(version)s.tbz2"),
    ("V_VERSION_TGZ", "v%(version)s.tgz"),
    ("V_VERSION_TXZ", "v%(version)s.txz"),
    ("V_VERSION_XZ", "v%(version)s.xz"),
    ("V_VERSION_ZIP", "v%(version)s.zip"),
    (
        "XORG_DATA_SOURCE",
        "https://xorg.freedesktop.org/archive/individual/data/",
    ),
    (
        "XORG_LIB_SOURCE",
        "https://xorg.freedesktop.org/archive/individual/lib/",
    ),
    (
        "XORG_PROTO_SOURCE",
        "https://xorg.freedesktop.org/archive/individual/proto/",
    ),
    (
        "XORG_UTIL_SOURCE",
        "https://xorg.freedesktop.org/archive/individual/util/",
    ),
    (
        "XORG_XCB_SOURCE",
        "https://xorg.freedesktop.org/archive/individual/xcb/",
    ),
];

#[cfg(target_os = "macos")]
const SHLIB_EXT: &str = "dylib";
#[cfg(target_os = "windows")]
const SHLIB_EXT: &str = "dll";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const SHLIB_EXT: &str = "so";

/// EasyBuild ALTERNATIVE_EASYCONFIG_TEMPLATE_CONSTANTS (same values as the primaries).
/// Extra historical spellings (`GITHUB_LOWER_URL`, `PYPI_LOWER_URL`) stay beside
/// the EasyBuild names (`GITHUB_URL_LOWER`, `PYPI_URL_LOWER`).
pub const EB_TEMPLATE_CONSTANT_ALIASES: &[(&str, &str)] = &[
    ("APACHE_URL", "APACHE_SOURCE"),
    ("BITBUCKET_GET_URL", "BITBUCKET_SOURCE"),
    ("BITBUCKET_DOWNLOADS_URL", "BITBUCKET_DOWNLOADS"),
    ("CRAN_URL", "CRAN_SOURCE"),
    ("FTP_GNOME_URL", "FTPGNOME_SOURCE"),
    ("GITHUB_URL", "GITHUB_SOURCE"),
    ("GITHUB_URL_LOWER", "GITHUB_LOWER_SOURCE"),
    ("GITHUB_LOWER_URL", "GITHUB_LOWER_SOURCE"),
    ("GITHUB_RELEASE_URL", "GITHUB_RELEASE"),
    ("GITHUB_RELEASE_URL_LOWER", "GITHUB_LOWER_RELEASE"),
    ("GNU_SAVANNAH_URL", "GNU_SAVANNAH_SOURCE"),
    ("GNU_FTP_URL", "GNU_FTP_SOURCE"),
    ("GNU_URL", "GNU_SOURCE"),
    ("GOOGLECODE_URL", "GOOGLECODE_SOURCE"),
    ("LAUNCHPAD_URL", "LAUNCHPAD_SOURCE"),
    ("PYPI_URL", "PYPI_SOURCE"),
    ("PYPI_URL_LOWER", "PYPI_LOWER_SOURCE"),
    ("PYPI_LOWER_URL", "PYPI_LOWER_SOURCE"),
    ("R_URL", "R_SOURCE"),
    ("SOURCEFORGE_URL", "SOURCEFORGE_SOURCE"),
    ("XORG_DATA_URL", "XORG_DATA_SOURCE"),
    ("XORG_LIB_URL", "XORG_LIB_SOURCE"),
    ("XORG_PROTO_URL", "XORG_PROTO_SOURCE"),
    ("XORG_UTIL_URL", "XORG_UTIL_SOURCE"),
    ("XORG_XCB_URL", "XORG_XCB_SOURCE"),
    ("SOURCE_LOWER_TAR_GZ", "SOURCELOWER_TAR_GZ"),
    ("SOURCE_LOWER_TAR_XZ", "SOURCELOWER_TAR_XZ"),
    ("SOURCE_LOWER_TAR_BZ2", "SOURCELOWER_TAR_BZ2"),
    ("SOURCE_LOWER_TGZ", "SOURCELOWER_TGZ"),
    ("SOURCE_LOWER_TXZ", "SOURCELOWER_TXZ"),
    ("SOURCE_LOWER_TBZ2", "SOURCELOWER_TBZ2"),
    ("SOURCE_LOWER_TB2", "SOURCELOWER_TB2"),
    ("SOURCE_LOWER_GTGZ", "SOURCELOWER_GTGZ"),
    ("SOURCE_LOWER_ZIP", "SOURCELOWER_ZIP"),
    ("SOURCE_LOWER_TAR", "SOURCELOWER_TAR"),
    ("SOURCE_LOWER_XZ", "SOURCELOWER_XZ"),
    ("SOURCE_LOWER_TAR_Z", "SOURCELOWER_TAR_Z"),
    ("SOURCE_LOWER_WHL", "SOURCELOWER_WHL"),
    ("SOURCE_LOWER_PY2_WHL", "SOURCELOWER_PY2_WHL"),
    ("SOURCE_LOWER_PY3_WHL", "SOURCELOWER_PY3_WHL"),
];

/// How many template constants the table carries, for a coverage assertion.
pub const EB_TEMPLATE_CONSTANTS_COUNT: usize = 78;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_constants_count_matches_table() {
        assert_eq!(EB_TEMPLATE_CONSTANTS.len(), EB_TEMPLATE_CONSTANTS_COUNT);
    }
}
