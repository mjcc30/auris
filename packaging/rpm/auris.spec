# Spec RPM pour Auris — construit en CI (job `rpm` de .github/workflows/release.yml)
# à partir de l'asset linux tar.gz. Version mise à jour par update-spec.py.
# Construction manuelle :
#   mkdir -p ~/rpmbuild/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
#   cp auris-x86_64-unknown-linux-gnu.tar.gz ~/rpmbuild/SOURCES/auris-<ver>.tar.gz
#   cp packaging/rpm/auris.service ~/rpmbuild/SOURCES/
#   python3 packaging/rpm/update-spec.py vX.Y.Z
#   rpmbuild -bb ~/rpmbuild/SPECS/auris.spec
Name:           auris
Version:        0.8.0
Release:        1%{?dist}
Summary:        Lecteur audiophile style Volumio
License:        AGPL-3.0-only
URL:            https://auris.audio
Source0:        %{name}-%{version}.tar.gz

Requires:       alsa-lib
Recommends:     ffmpeg

%description
Auris est un lecteur audiophile open source (AGPL-3.0) en un seul binaire Rust :
bibliotheque musicale indexee, lecture gapless et crossfade, DSD jusqu'au DSD256
en DoP, radios internet, sources NAS/UPnP/YouTube/Jamendo, multiroom, plugins
communautaires. Interface web embarquee sur http://<hote>:3000/.

%prep
%setup -q

%build
# Binaire deja compile (asset de release GitHub).

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}/usr/local/bin \
         %{buildroot}/etc/systemd/system \
         %{buildroot}/var/lib/auris
install -m 755 auris %{buildroot}/usr/local/bin/auris
install -m 644 %{_sourcedir}/auris.service %{buildroot}/etc/systemd/system/auris.service

%files
/usr/local/bin/auris
/etc/systemd/system/auris.service
%dir /var/lib/auris

%post
if ! id auris >/dev/null 2>&1; then
  useradd --system --home /var/lib/auris --shell /usr/sbin/nologin auris
fi
chown -R auris:auris /var/lib/auris
if [ ! -f /var/lib/auris/config.toml ]; then
  cat > /var/lib/auris/config.toml <<'CFG'
host = "0.0.0.0"
port = 3000
volume = 80
auto_update = true
repo_owner = "mjcc30"
repo_name = "auris"

[[sources]]
id = "nas"
name = "NAS"
enabled = true
kind = "local"
path = "/mnt/nas/musique"
CFG
  chown auris:auris /var/lib/auris/config.toml
  chmod 640 /var/lib/auris/config.toml
fi
systemctl daemon-reload || true
systemctl enable auris.service || true
systemctl restart auris.service || true

%preun
if [ "$1" = "0" ]; then
  systemctl stop auris.service || true
  systemctl disable auris.service || true
fi

%postun
systemctl daemon-reload || true
if [ "$1" = "0" ]; then
  userdel auris || true
  rm -rf /var/lib/auris
fi
