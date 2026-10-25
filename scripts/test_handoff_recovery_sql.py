"""Local PostgreSQL recovery regression using temporary tables and rollback.
Requires the Docker Compose PostgreSQL container; does not change application rows.
"""
from pathlib import Path
import re
import subprocess
root=Path(__file__).resolve().parents[1]
source=(root/'crates/api/src/state.rs').read_text()
queries=re.findall(r'"(INSERT INTO operator_alerts [^"]+)"', source)
assert len(queries)==4, len(queries)
sql='''BEGIN;
CREATE TEMP TABLE operator_alerts (id text PRIMARY KEY, kind text NOT NULL, session_id text NOT NULL, volunteer_id text, message text NOT NULL, created_at timestamptz NOT NULL);
CREATE TEMP TABLE sessions (id text PRIMARY KEY, escalation_count integer NOT NULL, updated_at timestamptz NOT NULL);
'''
for i,q in enumerate(queries):
    sql+=f'PREPARE recovery{i} AS {q};\n'
sql+='''EXECUTE recovery0('escalated:live', 'escalated', 'live', NULL, 'support', NOW());
EXECUTE recovery0('escalated:live', 'escalated', 'live', NULL, 'support', NOW());
EXECUTE recovery1('live', 3, NOW());
EXECUTE recovery1('live', 3, NOW());
INSERT INTO sessions VALUES ('ended', 2, NOW());
EXECUTE recovery2;
EXECUTE recovery3;
EXECUTE recovery2;
EXECUTE recovery3;
DO $$ BEGIN
IF (SELECT COUNT(*) FROM operator_alerts) <> 5 THEN RAISE EXCEPTION 'Recovery lost or duplicated alerts'; END IF;
END $$;
INSERT INTO operator_alerts VALUES ('legacy-uuid','escalated','legacy',NULL,'legacy alert',NOW());
EXECUTE recovery0('escalated:legacy','escalated','legacy',NULL,'support',NOW());
DO $$ BEGIN
IF (SELECT COUNT(*) FROM operator_alerts WHERE session_id='legacy') <> 1 THEN RAISE EXCEPTION 'Duplicated legacy alert'; END IF;
END $$;
ROLLBACK;
'''
result=subprocess.run(['docker','exec','-i','csc3104-sayangcare-postgres-1','psql','-U','sayangcare','-d','sayangcare','-v','ON_ERROR_STOP=1'],input=sql,text=True,capture_output=True)
print(result.stdout)
print(result.stderr)
raise SystemExit(result.returncode)
