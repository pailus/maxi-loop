-- 0002_drop_events_partition.sql
-- Adds a SECURITY DEFINER helper callable by `maxi_loop_worker` to drop daily child partitions
-- of `public.events` whose partition date is strictly older than `cutoff_date`.

CREATE OR REPLACE FUNCTION drop_expired_events_partitions(cutoff_date DATE)
RETURNS TEXT[]
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    rec RECORD;
    part_date DATE;
    dropped TEXT[] := ARRAY[]::TEXT[];
BEGIN
    IF cutoff_date IS NULL THEN
        RAISE EXCEPTION 'cutoff_date must not be null';
    END IF;

    FOR rec IN
        SELECT c.relname::TEXT AS part_name
        FROM pg_inherits i
        JOIN pg_class c ON c.oid = i.inhrelid
        JOIN pg_class p ON p.oid = i.inhparent
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND p.relname = 'events'
          AND c.relname ~ '^events_[0-9]{4}_[0-9]{2}_[0-9]{2}$'
        ORDER BY c.relname ASC
    LOOP
        part_date := to_date(substring(rec.part_name FROM 8 FOR 10), 'YYYY_MM_DD');
        IF part_date < cutoff_date THEN
            EXECUTE format('DROP TABLE IF EXISTS public.%I', rec.part_name);
            dropped := array_append(dropped, rec.part_name);
        END IF;
    END LOOP;

    RETURN dropped;
END;
$$;

REVOKE ALL ON FUNCTION drop_expired_events_partitions(DATE) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION drop_expired_events_partitions(DATE) TO maxi_loop_worker;
