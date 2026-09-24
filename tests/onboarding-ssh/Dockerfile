FROM debian:bookworm-slim

ENV container=docker

RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        ca-certificates curl openssh-server python3 sudo systemd systemd-sysv \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/bash lxcup \
    && usermod --append --groups sudo lxcup \
    && printf 'lxcup ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/lxcup \
    && chmod 0440 /etc/sudoers.d/lxcup \
    && mkdir -p /run/sshd \
    && mkdir -p /etc/systemd/system/multi-user.target.wants \
    && ln -sf /lib/systemd/system/ssh.service /etc/systemd/system/multi-user.target.wants/ssh.service \
    && sed -ri 's/^#?PasswordAuthentication .*/PasswordAuthentication yes/' /etc/ssh/sshd_config \
    && sed -ri 's/^#?KbdInteractiveAuthentication .*/KbdInteractiveAuthentication no/' /etc/ssh/sshd_config \
    && printf '\nUsePAM yes\n' >> /etc/ssh/sshd_config

COPY tests/onboarding-ssh/entrypoint.sh /usr/local/sbin/onboarding-ssh-entrypoint
RUN chmod 0755 /usr/local/sbin/onboarding-ssh-entrypoint

STOPSIGNAL SIGRTMIN+3
ENTRYPOINT ["/usr/local/sbin/onboarding-ssh-entrypoint"]
