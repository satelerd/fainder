class Fainder < Formula
  desc "Live universal finder for local AI agent conversations"
  homepage "https://github.com/satelerd/fainder"
  url "https://github.com/satelerd/fainder/archive/refs/tags/v0.1.3.tar.gz"
  sha256 "eec77608158aa3015adebf714e19b2c32489c4970e83b3a62764d502f35720cc"
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
