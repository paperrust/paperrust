#!/usr/bin/env bash

# Copyright (c) 2021-2026 community-scripts ORG
# Author: PaperRust (paperrust)
# License: MIT | https://github.com/community-scripts/DevScripts/raw/main/LICENSE
# Source: https://paperrust.org | Github: https://github.com/paperrust/paperrust

source /dev/stdin <<<"$FUNCTIONS_FILE_PATH"
color
verb_ip6
catch_errors
setting_up_container
network_check
update_os

msg_info "Installing Dependencies"
$STD apt install -y \
  tesseract-ocr \
  tesseract-ocr-eng \
  tesseract-ocr-deu \
  fonts-liberation
msg_ok "Installed Dependencies"

PG_VERSION="17" setup_postgresql
PG_DB_NAME="paperless" PG_DB_USER="paperless" setup_postgresql_db
fetch_and_deploy_gh_release "paperrust" "paperrust/paperrust" "prebuild" "latest" "/opt/paperrust" "PaperRust-*-linux-x86_64.tar.gz"

msg_info "Configuring PaperRust"
mkdir -p /opt/paperrust_data/{consume,data,media,export}
cat <<EOF >/etc/paperrust.conf
PAPERRUST_DBENGINE=postgresql
PAPERRUST_DBHOST=localhost
PAPERRUST_DBPORT=5432
PAPERRUST_DBNAME=${PG_DB_NAME}
PAPERRUST_DBUSER=${PG_DB_USER}
PAPERRUST_DBPASS=${PG_DB_PASS}
PAPERRUST_SECRET_KEY=$(random_password 64)
PAPERRUST_BIND=0.0.0.0:8000
PAPERRUST_TIME_ZONE=Etc/UTC
PAPERRUST_OCR_LANGUAGE=eng
PAPERRUST_DATA_DIR=/opt/paperrust_data/data
PAPERRUST_MEDIA_ROOT=/opt/paperrust_data/media
PAPERRUST_CONSUMPTION_DIR=/opt/paperrust_data/consume
PAPERRUST_STATICDIR=/opt/paperrust/static
PAPERRUST_ROUTES=/opt/paperrust/routes.toml
PAPERRUST_PDFIUM_PATH=/opt/paperrust/pdfium/libpdfium.so
PAPERRUST_TESSERACT_PATH=/usr/bin/tesseract
EOF
chmod 600 /etc/paperrust.conf
msg_ok "Configured PaperRust"

msg_info "Setting up PaperRust Database"
ADMIN_PASS=$(random_password)
cd /opt/paperrust
$STD /opt/paperrust/paperrust manage migrate
PAPERRUST_ADMIN_USER=admin PAPERRUST_ADMIN_PASSWORD="$ADMIN_PASS" $STD /opt/paperrust/paperrust manage manage_superuser
$STD /opt/paperrust/paperrust manage document_index reindex
cat <<EOF >>~/paperrust.creds

PaperRust WebUI User: admin
PaperRust WebUI Password: ${ADMIN_PASS}
EOF
msg_ok "Set up PaperRust Database"

msg_info "Creating Service"
cat <<EOF >/etc/systemd/system/paperrust.service
[Unit]
Description=PaperRust
After=network.target postgresql.service
Wants=postgresql.service

[Service]
Type=simple
User=root
WorkingDirectory=/opt/paperrust
EnvironmentFile=/etc/paperrust.conf
Environment=PAPERRUST_RESTART_SUPERVISED=1
ExecStart=/opt/paperrust/paperrust serve --worker
Restart=on-failure
RestartSec=5
RestartForceExitStatus=75
TimeoutStopSec=45

[Install]
WantedBy=multi-user.target
EOF
systemctl enable -q --now paperrust
msg_ok "Created Service"

motd_ssh
customize
cleanup_lxc
