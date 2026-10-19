# Homebrew formula TEMPLATE. Not submitted: replace VERSION and the SHA256_*
# placeholders from the release's SHA256SUMS before use (see ../README.md).
class Tpt < Formula
  desc "Command-line tool for the TPT media codecs"
  homepage "https://github.com/TPT-Solutions/tpt-media"
  version "VERSION"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/TPT-Solutions/tpt-media/releases/download/vVERSION/tpt-macos-aarch64"
      sha256 "SHA256_MACOS_AARCH64"
    else
      url "https://github.com/TPT-Solutions/tpt-media/releases/download/vVERSION/tpt-macos-x86_64"
      sha256 "SHA256_MACOS_X86_64"
    end
  end

  on_linux do
    url "https://github.com/TPT-Solutions/tpt-media/releases/download/vVERSION/tpt-linux-x86_64"
    sha256 "SHA256_LINUX_X86_64"
  end

  def install
    bin.install Dir["tpt-*"].first => "tpt"
  end

  test do
    assert_match "USAGE", shell_output("#{bin}/tpt --help")
    system bin/"tpt", "doctor"
  end
end
