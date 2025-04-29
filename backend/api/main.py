import fastapi
import logging
from fastapi.middleware.cors import CORSMiddleware

app = fastapi.FastAPI()

# Allow CORS for frontend
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

logging.basicConfig(level=logging.INFO)

@app.get("/")
def read_root():
    return {"message": "Hello World from backend!"}

@app.post("/log-frontend-message")
def log_frontend_message():
    logging.info("hello backend world from front-end")
    return {"status": "logged"}
