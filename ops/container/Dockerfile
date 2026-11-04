FROM rust:1.93.0-slim-bookworm AS rust-toolchain
RUN mkdir /toolchain && cp -a "$(rustc --print sysroot)/." /toolchain/

FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git gh python3 python3-venv python3-pip python3-pam \
    nodejs npm build-essential pkg-config libssl-dev libpam0g libpam0g-dev bubblewrap \
    tini iproute2 && rm -rf /var/lib/apt/lists/*
COPY --from=rust-toolchain /toolchain/bin/ /usr/local/bin/
COPY --from=rust-toolchain /toolchain/lib/ /usr/local/lib/
# The uid is supplied by the installation; no login password is stored here.
RUN usermod -l agent ubuntu && groupmod -n agent ubuntu && usermod -d /home/agent agent
COPY pam-client.py entrypoint.py service.py /opt/constellation/
RUN printf 'auth required pam_exec.so quiet expose_authtok /usr/bin/python3 /opt/constellation/pam-client.py\naccount required pam_permit.so\n' > /etc/pam.d/agent-instance \
    && cp /etc/pam.d/agent-instance /etc/pam.d/acp-slack-adapter
USER 1000:1000
ENV HOME=/home/agent LANG=C.UTF-8
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/bin/python3", "/opt/constellation/entrypoint.py"]
