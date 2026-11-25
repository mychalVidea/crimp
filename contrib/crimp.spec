Name:           crimp
Version:        0.2.0
Release:        1%{?dist}
Summary:        Ultralehký PipeWire Patchbay GUI, routing daemon a live DSP manažer

License:        MIT
URL:            https://github.com/mychalVidea/crimp
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz

BuildRequires:  rust >= 1.100
BuildRequires:  cargo
BuildRequires:  clang
BuildRequires:  pkgconfig(libpipewire-0.3)
Requires:       pipewire-libs

%description
Crimp chrání PipeWire audio graf před nechtěným přepojováním WirePlumberu,
spravuje virtuální sběrnice, dynamické DSP profily a přináší nativní
lehké Patchbay GUI (crimp-ui) jako alternativu ke qpwgraph.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --workspace --locked

%install
install -Dm755 target/release/crimp %{buildroot}%{_bindir}/crimp
install -Dm755 target/release/crimp-ui %{buildroot}%{_bindir}/crimp-ui
install -Dm644 contrib/crimp.desktop %{buildroot}%{_datadir}/applications/crimp.desktop
install -Dm644 crimp.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/crimp.svg
install -Dm644 contrib/crimp.service %{buildroot}%{_userunitdir}/crimp.service

%files
%license LICENSE
%doc README.md
%{_bindir}/crimp
%{_bindir}/crimp-ui
%{_datadir}/applications/crimp.desktop
%{_datadir}/icons/hicolor/scalable/apps/crimp.svg
%{_userunitdir}/crimp.service
