#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}🚀 Starting Citadel WebSocket TypeScript Integration Tests${NC}"
echo ""

# Get the git repository root directory
PROJECT_ROOT="$(git rev-parse --show-toplevel)"
cd "$PROJECT_ROOT"

echo -e "${YELLOW}📁 Git repository root: $PROJECT_ROOT${NC}"

# Function to cleanup background processes
cleanup() {
    echo -e "\n${YELLOW}🧹 Cleaning up...${NC}"
    if [ ! -z "$SERVER_PID" ]; then
        echo -e "${YELLOW}🔪 Killing server process $SERVER_PID${NC}"
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true
    fi
    exit
}

# Setup cleanup trap
trap cleanup INT TERM EXIT

# Step 1: Generate TypeScript types
echo -e "${BLUE}🔧 Step 1: Generating TypeScript types...${NC}"
cd citadel-internal-service-types
cargo run --features typescript --example generate_complete_types
cd "$PROJECT_ROOT"

# Check if types were generated in project root
if [ ! -d "bindings" ]; then
    echo -e "${RED}❌ TypeScript types were not generated!${NC}"
    exit 1
fi

echo -e "${GREEN}✅ TypeScript types generated successfully${NC}"
echo ""

# Step 2: Setup TypeScript project
echo -e "${BLUE}🔧 Step 2: Setting up TypeScript project...${NC}"
cd typescript-client

# Install dependencies if node_modules doesn't exist
if [ ! -d "node_modules" ]; then
    echo -e "${YELLOW}📦 Installing npm dependencies...${NC}"
    npm install
fi

echo -e "${GREEN}✅ TypeScript project setup complete${NC}"
echo ""

# Step 3: Start Rust WebSocket server
echo -e "${BLUE}🚀 Step 3: Starting Rust WebSocket server...${NC}"
cd "$PROJECT_ROOT"

# Build and start server in background
cargo run --features websockets --example websocket_server &
SERVER_PID=$!

echo -e "${YELLOW}🔄 Server PID: $SERVER_PID${NC}"
echo -e "${YELLOW}⏳ Waiting for server to start...${NC}"

# Wait for server to be ready
sleep 8

# Check if server is still running
if ! kill -0 $SERVER_PID 2>/dev/null; then
    echo -e "${RED}❌ Server failed to start!${NC}"
    exit 1
fi

echo -e "${GREEN}✅ Server started successfully${NC}"
echo ""

# Step 4: Build and run TypeScript tests
echo -e "${BLUE}🧪 Step 4: Running TypeScript tests...${NC}"
cd typescript-client

# Build TypeScript project
echo -e "${YELLOW}🔨 Building TypeScript project...${NC}"
npm run build

if [ $? -ne 0 ]; then
    echo -e "${RED}❌ TypeScript build failed!${NC}"
    exit 1
fi

echo -e "${GREEN}✅ TypeScript build successful${NC}"

# Run the tests
echo -e "${YELLOW}🧪 Running integration tests...${NC}"
npm run test

if [ $? -eq 0 ]; then
    echo ""
    echo -e "${GREEN}🎉 All tests passed successfully!${NC}"
    echo -e "${GREEN}✅ Rust WebSocket server and TypeScript client integration working!${NC}"
else
    echo ""
    echo -e "${RED}❌ Tests failed!${NC}"
    exit 1
fi

echo ""
echo -e "${BLUE}📊 Test Summary:${NC}"
echo -e "${GREEN}  ✅ TypeScript type generation${NC}"
echo -e "${GREEN}  ✅ WebSocket server startup${NC}"  
echo -e "${GREEN}  ✅ TypeScript client build${NC}"
echo -e "${GREEN}  ✅ End-to-end communication${NC}"
echo ""
echo -e "${BLUE}🎯 Integration test completed successfully!${NC}" 