class Huck < Formula
  desc "POSIX-ish shell written in Rust"
  homepage "https://github.com/jdstanhope/huck"
  url "https://github.com/jdstanhope/huck/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"
  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_equal "hi\n", shell_output("#{bin}/huck -c 'echo hi'")
  end
end
