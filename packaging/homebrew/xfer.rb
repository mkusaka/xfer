class Xfer < Formula
  desc "Prepare a bounded session handoff for another coding agent"
  homepage "https://github.com/mkusaka/xfer"
  version "__VERSION__"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "__DARWIN_ARM64_URL__"
      sha256 "__DARWIN_ARM64_SHA256__"
    else
      url "__DARWIN_X64_URL__"
      sha256 "__DARWIN_X64_SHA256__"
    end
  end

  def install
    bin.install "xfer"
    (pkgshare/"skills").install Dir["skills/*"]
  end

  def caveats
    <<~EOS
      Agent skills were installed to:
        #{opt_pkgshare}/skills

      Install them with npx skills:
        npx -y skills add "#{opt_pkgshare}/skills" -y --copy

      Or install directly from the repository:
        npx -y skills add https://github.com/mkusaka/xfer -y
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/xfer --version")
  end
end
