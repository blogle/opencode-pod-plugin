FROM python:3.13.5-slim-bookworm
COPY tests/fixtures/fake-llm/server.py /server.py
EXPOSE 8080
ENTRYPOINT ["python3", "/server.py"]
