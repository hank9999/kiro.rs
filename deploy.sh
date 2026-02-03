docker build -t fqdeng/kiro-rs:latest . && docker save | ssh root@us3.jonwinters.pw 'docker load && cd /root/kiro.rs && docker compose down && docker compose up -d'
