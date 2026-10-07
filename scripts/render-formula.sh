#!/bin/sh
# Usage: render-formula.sh <version> <dir with *.tar.gz.sha256> > buzz-backend-docker.rb
set -eu
version=$1
dist=$2
base="https://github.com/abipalli/buzz-backend-docker/releases/download/v$version"

asset() {
  name="buzz-backend-docker-v$version-$1.tar.gz"
  sum=$(cut -d' ' -f1 "$dist/$name.sha256")
  printf '      url "%s/%s"\n      sha256 "%s"\n' "$base" "$name" "$sum"
}

cat <<FORMULA
class BuzzBackendDocker < Formula
  desc "Run Buzz agents on your own server: remote-agent backend for any Docker host"
  homepage "https://github.com/abipalli/buzz-backend-docker"
  version "$version"
  license "Apache-2.0"

  on_macos do
    on_arm do
$(asset aarch64-apple-darwin)
    end
    on_intel do
$(asset x86_64-apple-darwin)
    end
  end

  on_linux do
    on_arm do
$(asset aarch64-unknown-linux-musl)
    end
    on_intel do
$(asset x86_64-unknown-linux-musl)
    end
  end

  def install
    bin.install "buzz-backend-docker"
  end

  def caveats
    <<~EOS
      Finish with one command (links the provider where Buzz Desktop looks and
      checks your server):

        buzz-backend-docker setup ssh://you@your-server
    EOS
  end

  test do
    assert_match '"protocol_version":1', pipe_output(bin/"buzz-backend-docker", '{"op":"info"}')
  end
end
FORMULA
