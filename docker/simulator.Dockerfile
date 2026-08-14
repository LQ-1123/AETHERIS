# DCMTK 设备模拟器容器：浏览器 UI + storescu 并发上传到 PACS。
# 模拟器在本机运行时可省略本服务（python3 tools/dcmtk-simulator.py）。
FROM python:3.12-slim-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends dcmtk \
    && rm -rf /var/lib/apt/lists/*

COPY tools/dcmtk-simulator.py tools/dcmtk-simulator.html /tools/

WORKDIR /tools
EXPOSE 8787
CMD ["python3", "/tools/dcmtk-simulator.py"]
