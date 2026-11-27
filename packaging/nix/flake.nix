# Flake Nix pour Auris — empaquette le binaire de release (comme la formule Homebrew).
# Usage :
#   nix run github:mjcc30/auris/packaging/nix#auris -- serve --port 3000   (non, voir ci-dessous)
#   nix build ./packaging/nix#auris
#   nix run ./packaging/nix#auris -- serve --port 3000
# Version/sha256 mis à jour par packaging/nix/update-nix.py à chaque release.
{
  description = "Auris — lecteur audiophile style Volumio (binaire de release)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      version = "0.8.0";
      assets = {
        x86_64-linux = {
          file = "auris-x86_64-unknown-linux-gnu.tar.gz";
          sha256 = "cdf11327a588de4bfdf59243067499116c4b15a71b3421deab40a244537abe0e";
        };
        aarch64-linux = {
          file = "auris-aarch64-unknown-linux-gnu.tar.gz";
          sha256 = "4b784f1e8391a148ca96989d21f958abc94b656cbef5b5d1def98954598748b0";
        };
        aarch64-darwin = {
          file = "auris-aarch64-apple-darwin.tar.gz";
          sha256 = "9bb664732bbc9b15550cf2c40c4456866fd44bd94303948d70c24f7b9855fb24";
        };
      };
      systems = builtins.attrNames assets;
      mk =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          asset = assets.${system};
        in
        pkgs.stdenv.mkDerivation {
          pname = "auris";
          inherit version;
          src = pkgs.fetchurl {
            url = "https://github.com/mjcc30/auris/releases/download/v${version}/${asset.file}";
            inherit (asset) sha256;
          };
          nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.autoPatchelfHook ];
          buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.alsa-lib
            pkgs.openssl
            pkgs.stdenv.cc.cc.lib # libgcc_s.so.1 (requis par autoPatchelfHook)
          ];
          sourceRoot = ".";
          installPhase = ''
            runHook preInstall
            install -Dm755 auris "$out/bin/auris"
            runHook postInstall
          '';
          meta = with pkgs.lib; {
            description = "Lecteur audiophile style Volumio (gapless, DSD/DoP, radios, multiroom)";
            homepage = "https://auris.audio";
            license = licenses.agpl3Only;
            platforms = platforms.linux ++ platforms.darwin;
            mainProgram = "auris";
          };
        };
    in
    {
      packages = nixpkgs.lib.genAttrs systems (s: {
        auris = mk s;
        default = mk s;
      });
      apps = nixpkgs.lib.genAttrs systems (s: {
        auris = {
          type = "app";
          program = "${self.packages.${s}.auris}/bin/auris";
        };
        default = self.apps.${s}.auris;
      });
    };
}
