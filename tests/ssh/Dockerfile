FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server python3 util-linux && rm -rf /var/lib/apt/lists/* && useradd -m -s /bin/sh fixture && passwd -d fixture && mkdir -p /run/sshd /home/fixture/.ssh && chmod 700 /home/fixture/.ssh
COPY authorized_keys /home/fixture/.ssh/authorized_keys
RUN chown -R fixture:fixture /home/fixture/.ssh && chmod 600 /home/fixture/.ssh/authorized_keys && ssh-keygen -A && printf 'PasswordAuthentication no\nAllowUsers fixture\nLogLevel ERROR\n' >> /etc/ssh/sshd_config
CMD ["/usr/sbin/sshd", "-D", "-e"]
