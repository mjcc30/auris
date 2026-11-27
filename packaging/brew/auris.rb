# Formula Homebrew pour Auris — à copier dans un tap (ex. `mjcc30/homebrew-auris`) :
#   gh repo create mjcc30/homebrew-auris --public
#   cp packaging/brew/auris.rb <tap>/Formula/auris.rb
# Le sha256 ci-dessous est celui de la release v0.8.0 (asset aarch64-apple-darwin) ;
# `packaging/brew/update-formula.py vX.Y.Z` le met à jour automatiquement.
class Auris < Formula
  desc "Lecteur audiophile style Volumio (binaire Rust unique, UI web embarquée, gapless/DoP)"
  homepage "https://auris.audio"
  version "0.8.0"
  license "AGPL-3.0-only"

  on_macos do
    on_arm do
      url "https://github.com/mjcc30/auris/releases/download/v0.8.0/auris-aarch64-apple-darwin.tar.gz"
      sha256 "9bb664732bbc9b15550cf2c40c4456866fd44bd94303948d70c24f7b9855fb24"
    end
  end

  depends_on "ffmpeg" # repli de décodage (Opus/WMA/APE/TTA…) hors symphonia

  def install
    bin.install "auris"
  end

  def post_install
    (var/"auris").mkpath
  end

  service do
    run [opt_bin/"auris", "serve", "--data-dir", var/"auris", "--port", "3000"]
    run_type :immediate
    keep_alive true
    working_dir var
    log_path var/"log/auris.log"
    error_log_path var/"log/auris.err.log"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/auris version")
  end
end
