IMAGE_NAME := xcc
BINARY_PATH := target/aarch64-unknown-linux-gnu/release/mbbs
ENV_FILE := .env

# Load variables from .env if it exists
ifneq ("$(wildcard $(ENV_FILE))","")
include $(ENV_FILE)
export $(shell sed 's/=.*//' $(ENV_FILE))
endif

.PHONY: all build run deploy clean

all: build run deploy

build:
	@echo "==> Building Docker image '$(IMAGE_NAME)'..."
	@docker build . -t $(IMAGE_NAME)
	@echo "==> Docker image built successfully."

run:
	@echo "==> Running build container..."
	@mkdir -p target
	@docker run --rm -v $(PWD)/target:/build/target $(IMAGE_NAME)
	@echo "==> Build completed, output available in ./target"

deploy:
	@echo "==> Deploying binary to $(DEPLOYMENT_PATH)..."
	@timestamp=$$(date +"%Y-%m-%d-%H-%M"); \
	scp $(BINARY_PATH) $${DEPLOYMENT_PATH}_$${timestamp}
	@echo "==> Deployment completed successfully."

clean:
	@echo "==> Cleaning build artifacts..."
	@rm -rf target
	@echo "==> Clean complete."
