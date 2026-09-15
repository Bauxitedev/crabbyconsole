set -euxo pipefail
# ^ setup common error handling + print every command (-x)

# run this in your docker container...
apt update
apt install -y pipx

# setup appuser's home dir, or it spams errors related to bashrc
mkdir -p /home/appuser && chown 1000:1000 /home/appuser

rest_of_script() {
    echo "now running as $(id)"

    # setup PATH so we can use pipx/uv
    pipx ensurepath
    source ~/.bashrc
    pipx install "uv==0.12.9" # this is per-user

    uv sync # --locked
    echo "now run this:"
    echo "uv run zensical serve --dev-addr 0.0.0.0:8000"

    # drop into bash interactive shell afterwards
    exec bash -i
}
export -f rest_of_script

# switch to appuser and run the rest of the script as non-root
HOME=/home/appuser setpriv --reuid=1000 --regid=1000 --clear-groups bash -c rest_of_script

