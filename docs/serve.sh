# this is how to serve it in docker
# you need to bind to 0.0.0.0, not localhost, or you get a weird error when you try to open it in the browser
uv run zensical serve --dev-addr 0.0.0.0:8000
