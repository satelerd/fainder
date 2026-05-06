class Fainder < Formula
  desc "Live universal finder for local AI agent conversations"
  homepage "https://github.com/satelerd/fainder"
  url "https://github.com/satelerd/fainder/archive/refs/tags/v0.1.2.tar.gz"
  sha256 "15efb3e6b0348ab8a12b28b166e0f1b6065f7429163b13f61b0767248d3bd7bd"
  license "MIT"

  depends_on "rust" => :build
  depends_on "ripgrep"

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "Fainder doctor", shell_output("#{bin}/fainder doctor")
  end

  def caveats
    <<~EOS
      ╭────────────────────────────────────────╮
      │ May the agents vibe with you.          │
      ╰────────────────────────────────────────╯
    EOS
  end
end
