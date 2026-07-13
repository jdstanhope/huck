class Huck < Formula
  desc "POSIX-ish shell written in Rust"
  homepage "https://github.com/jdstanhope/huck"
  url "https://github.com/jdstanhope/huck/archive/refs/tags/v0.3.0.tar.gz"
  sha256 "528b63b1cbb8e00ec795d0f1930590885c31dce4a1db5c8926423a0ae33edc18"
  license "MIT"
  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_equal "hi\n", shell_output("#{bin}/huck -c 'echo hi'")
  end
end
