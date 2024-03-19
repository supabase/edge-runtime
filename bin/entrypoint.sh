#!/bin/bash
set -eu

export PS1='\w $ '

EXTENSIONS="${CODE_SERVER_EXTENSIONS:-none}"

if [ -z "$USE_CODE_SERVER_INTEGRATION" ]; then
    edge-runtime $@ &
else
    mkdir -p /root/.local/share/code-server/User
    cat > /root/.local/share/code-server/User/settings.json << EOF
    {
        "workbench.colorTheme": "Visual Studio Dark",
        "deno.enable": true
    }
EOF

    if [ ${EXTENSIONS} != "none" ]; then
        echo "Installing Extensions"
        for extension in $(echo ${EXTENSIONS} | tr "," "\n")
        do
            if [ "${extension}" != "" ]; then
                /usr/bin/code-server \
                    --install-extension "${extension}" \
                    /home/deno/functions
            fi
        done
    fi

    (/usr/bin/code-server \
        --bind-addr "${CODE_SERVER_HOST}":"${CODE_SERVER_PORT}" \
        --auth none \
        /home/deno/functions \
    ) &

    edge-runtime $@ &
fi

wait -n