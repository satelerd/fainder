class Fainder < Formula
  desc "Live universal finder for local AI agent conversations"
  homepage "https://github.com/satelerd/fainder"
  url "https://github.com/satelerd/fainder/archive/refs/tags/v0.1.5.tar.gz"
  sha256 "c0cd023de8cc7ede032b381b12cb3f9f86574ce1fd9c380b5ecff7cba216c227"
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
