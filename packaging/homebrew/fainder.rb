class Fainder < Formula
  desc "Live universal finder for local AI agent conversations"
  homepage "https://github.com/satelerd/fainder"
  url "https://github.com/satelerd/fainder/archive/refs/tags/v0.1.4.tar.gz"
  sha256 "514bc05e1bbc4f116f374b0b071a798c04c7a54064178485c55c2d8e0b05b957"
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
