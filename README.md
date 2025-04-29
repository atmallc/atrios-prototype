ATRIOS

This document will first explain the author’s understanding of what Atrios is building, and proposes a 30 day roadmap (key goals, systems, features) and tech stack decision.

What is Atrios?
Atrios (getatrios.com) is building a new way to connect networks, monetize relationships, and scale revenue without relying on the traditional cold outbound playbook. They are pre-product, pre-seed, and moving fast. 

They have two customer types: 
CustomerA: Sales team from a company: Team that wants to sell their products/services to target company(s).
CustomerB: Connectors i.e. folks who have connections to the correct leads within the target company(s).

How Atrios works?
CustomerA upload their company leads on to Atrio's platform. 
CustomerA set's their maximum bid they'd like to spend for an introduction.
Atrio's AI will instantly match CustomerA with CustomerB.
Once a CustomerB accepts your bid, CustomerA preps them on how to pitch them to the target company.
CustomerA receives the intro via email and starts selling!
Payment is collected by CustomerB after successful intro

Example customer journey
Customer Profile: A fictional startup called Bumbble is selling a developer productivity tool (e.g., code documentation automation) priced at $20k/year for engineering teams.
Sales Goal: Book demos with engineering leadership at Series B+ tech companies in North America and has a list of 1000 potential leads.
Target Lead Example
Name: Julia Martinez
Title: VP of Engineering
Company: Clarity AI
Industry: Fintech / AI
Company Size: 200–500 employees
Location: New York, NY
Tech Stack: Uses GitHub, Slack, Notion
Pain Points:
Scaling dev team from 25 to 60
Onboarding new engineers quickly
Improving internal code documentation
Bumbble uploads the above lead on to Atrios platform and sets a bounty of $5000 estimating that a sale with Clarity can bring them $1.2 mil.
Atrios has a pool of 20 connectors, one of the connectors, Bill, is Julia Martinez's college friend.
Bill accepts the bounty and gets a communication from Atrios connecting him with Bumbble's sales team.
Bumbble and Bill take it from there, preping and getting introduced to Juila.
Once the introduction is sent to Julia (and email doesn't bounce) Bumbble will pay Bill for the intro.


Feature set for a Prototype built in 30 days

For Sales Teams (CustomerA):
Simple user authentication (email/password or magic link)
Basic dashboard to upload leads (CSV only, has to contain links to linkedin profile).
Manual entry of lead bounties
Basic list view of leads and status
Rule based match making
Email notifications for matches (no in-app messaging)
Stripe test integration

For Connectors (CustomerB):
Simple user authentication
Upload LinkedIn contact list (CSV only))
Basic list view of available bounties
Accept/decline via dashboard
Stripe test integration

Platform-wide:
Minimal admin tools (manual user/lead management)
Basic security (hashed passwords, HTTPS)
No advanced analytics or dashboards
No 2FA or advanced compliance (but data privacy best practices)
Simple rule-based matching logic based on LinkedIn profile url.
Focus on getting end-to-end flow working: upload → match → intro → payout

30-Day Roadmap

Week 1: Foundations & Planning
Finalize product requirements for prototype (scope: CSV upload, rule-based matching, Stripe test integration, basic dashboards).
Design wireframes for CustomerA (Sales) and CustomerB (Connector) dashboards.
Decide on tech stack and set up repos, CI/CD, and dev environments.
Implement basic user authentication (email/password or magic link).

Week 2: Core Data Flows
Build lead upload (CSV) and parsing for Sales teams.
Build contact upload (CSV) for Connectors.
Create basic dashboards:
Sales: view/manage leads, set bounties.
Connectors: view/manage contacts, see matching bounties.
Implement rule-based matching engine (by LinkedIn URL).

Week 3: End-to-End Flow
Integrate email notifications for matches and intro requests.
Add Stripe test-mode integration for bounty payouts.
Allow Connectors to accept/decline bounties via dashboard.
Implement minimal admin tools for manual user/lead management.

Week 4: Polish & Testing
Harden security (hashed passwords, HTTPS, data privacy best practices).
End-to-end flow testing: upload → match → intro → payout.
Collect feedback from alpha users (internal or friendly customers).
Prepare demo, documentation, and onboarding guides.

Key Tech Stack Decisions

What to Build In-House (Prototype):
Core web app: dashboards, CSV upload/parsing, matching logic, user auth.
Minimal admin tools and manual management.
Email notification system (can use third-party transactional email, e.g., SendGrid).
Stripe integration (test mode for now).

What to Outsource/Leverage:
Authentication: Use a managed service (e.g., Auth0, Clerk, or Firebase Auth) for rapid setup and security.
Email delivery: Use a transactional email API (SendGrid, Postmark, Mailgun).
Hosting: Use a managed PaaS (e.g., Vercel, Heroku, or AWS Amplify) for fast deploys and SSL.
Infrastructure: Use managed DB (e.g., Supabase, Firebase, or AWS RDS).

Tech Stack Recommendation:
Frontend: React (Next.js for SSR, easy deployment, and fast dev), TailwindCSS for UI.
Backend: Python - Fast API. For rapid iteration, consider serverless functions (Vercel, AWS Lambda).
Database: PostgreSQL (hosted on Supabase or AWS RDS)
Auth: Auth0
Payments: Stripe (test mode for prototype).
Email: SendGrid/Postmark.
Infra/Hosting: Vercel, or AWS Amplify or Heroku.
Other: GitHub Actions for CI/CD.


Tech Risks and Trade-offs
Data is expensive: LinkedIn strictly does not allow data scrapping. Eventually we will:
Partner with large data contributor network and and third-party data providers.
Build public data crawling,
Build proprietary tools to verify and validate the data.
Platform Dependency: Heavy reliance on third-party platforms (e.g., LinkedIn, email providers, Stripe) exposes you to API changes, rate limits, or bans, which could disrupt your service.
Scalability: Rapid growth in user base or data volume could strain infrastructure if not planned for, leading to downtime or degraded performance.
Technical Debt: Fast prototyping under tight deadlines can lead to shortcuts and technical debt, making future scaling and feature development harder.
Compliance & Privacy: Handling personal and professional contact data (especially LinkedIn connections) raises significant privacy and regulatory compliance risks (GDPR, CCPA, etc.). Any data breach or misuse could have serious legal and reputational consequences.

Product Risks
Quality Control: Ensuring the quality and relevance of introductions is difficult. Poor matches or low-quality intros could erode trust and reduce platform adoption.
Payment Disputes: Disagreements may arise over whether an intro was “successful” or if a bounty payout is warranted, leading to support overhead and potential legal issues.
User Onboarding & Retention: Convincing connectors and sales teams to join and remain active may be challenging, especially if early liquidity is low (chicken-and-egg problem).
Intellectual Property: Risk of competitors copying the model or features, especially if the platform gains traction.
Reputation Management: Negative experiences (spam, privacy complaints, failed intros) could spread quickly among target users, harming brand reputation.
Platform Abuse & Fraud: There is a risk of users gaming the system with fake connectors, fraudulent intros, or sales teams uploading low-quality/spam leads to exploit payouts.


Competitors
CoSell.io: Helps companies partner with others to share trusted intros for warm leads.
Apollo.io: Powerful B2B sales platform combining lead generation, enrichment, and outreach tools.
Warmly.ai: Helps reps warm up cold leads using mutual connections and real-time buyer intent signals.
Connect The Dots (https://www.ctd.ai/): Uses your email metadata and social graph to reveal who in your network can intro you to target leads.
ZoomInfo.com: Massive contact database with sales intelligence and automated workflows.