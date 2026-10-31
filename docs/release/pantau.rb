cask "pantau" do
  version "0.1.0"
  sha256 "REPLACE_ME"

  url "https://github.com/fizipan/pantau/releases/download/v#{version}/Pantau_#{version}_aarch64.dmg"
  name "Pantau"
  desc "Menu bar app that shows CPU, memory and disk load and cleans what is safe to clean"
  homepage "https://github.com/fizipan/pantau"

  depends_on arch: :arm64

  app "Pantau.app"

  # Not notarized: the app is signed ad hoc, so macOS would refuse to open it
  # while the download quarantine flag is set.
  postflight_steps do
    run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{appdir}}/Pantau.app"]
  end

  # The login item is a plain LaunchAgent file (no launchctl needed to remove it).
  uninstall quit:   "local.toolslab.pantau",
            delete: "~/Library/LaunchAgents/Pantau.plist"

  zap trash: [
    "~/Library/Application Support/local.toolslab.pantau",
    "~/Library/Caches/local.toolslab.pantau",
    "~/Library/Caches/pantau",
    "~/Library/WebKit/local.toolslab.pantau",
    "~/Library/WebKit/pantau",
  ]

  caveats <<~EOS
    Pantau is not notarized by Apple. This cask removes the download quarantine flag
    from the app so that macOS can open it. Source: https://github.com/fizipan/pantau

    Pantau can stop processes and delete cache folders, always after a click.
    This is an early release: read the confirmation before you click.
  EOS
end
