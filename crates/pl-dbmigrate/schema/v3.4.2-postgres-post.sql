-- paperless-ngx v3.4.2, Django-Schema auf PostgreSQL (post-data). Erzeugt von tests/dbmigrate/gen_schema.sh – nicht von Hand ändern.

ALTER TABLE ONLY public.account_emailaddress
    ADD CONSTRAINT account_emailaddress_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.account_emailaddress
    ADD CONSTRAINT account_emailaddress_user_id_email_987c8728_uniq UNIQUE (user_id, email);

ALTER TABLE ONLY public.account_emailconfirmation
    ADD CONSTRAINT account_emailconfirmation_key_key UNIQUE (key);

ALTER TABLE ONLY public.account_emailconfirmation
    ADD CONSTRAINT account_emailconfirmation_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auditlog_logentry
    ADD CONSTRAINT auditlog_logentry_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auth_group
    ADD CONSTRAINT auth_group_name_key UNIQUE (name);

ALTER TABLE ONLY public.auth_group_permissions
    ADD CONSTRAINT auth_group_permissions_group_id_permission_id_0cd325b0_uniq UNIQUE (group_id, permission_id);

ALTER TABLE ONLY public.auth_group_permissions
    ADD CONSTRAINT auth_group_permissions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auth_group
    ADD CONSTRAINT auth_group_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auth_permission
    ADD CONSTRAINT auth_permission_content_type_id_codename_01ab375a_uniq UNIQUE (content_type_id, codename);

ALTER TABLE ONLY public.auth_permission
    ADD CONSTRAINT auth_permission_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auth_user_groups
    ADD CONSTRAINT auth_user_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auth_user_groups
    ADD CONSTRAINT auth_user_groups_user_id_group_id_94350c0c_uniq UNIQUE (user_id, group_id);

ALTER TABLE ONLY public.auth_user
    ADD CONSTRAINT auth_user_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auth_user_user_permissions
    ADD CONSTRAINT auth_user_user_permissions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.auth_user_user_permissions
    ADD CONSTRAINT auth_user_user_permissions_user_id_permission_id_14a6b632_uniq UNIQUE (user_id, permission_id);

ALTER TABLE ONLY public.auth_user
    ADD CONSTRAINT auth_user_username_key UNIQUE (username);

ALTER TABLE ONLY public.authtoken_token
    ADD CONSTRAINT authtoken_token_pkey PRIMARY KEY (key);

ALTER TABLE ONLY public.authtoken_token
    ADD CONSTRAINT authtoken_token_user_id_key UNIQUE (user_id);

ALTER TABLE ONLY public.django_admin_log
    ADD CONSTRAINT django_admin_log_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.django_content_type
    ADD CONSTRAINT django_content_type_app_label_model_76bd3d3b_uniq UNIQUE (app_label, model);

ALTER TABLE ONLY public.django_content_type
    ADD CONSTRAINT django_content_type_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.django_migrations
    ADD CONSTRAINT django_migrations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.django_session
    ADD CONSTRAINT django_session_pkey PRIMARY KEY (session_key);

ALTER TABLE ONLY public.documents_correspondent
    ADD CONSTRAINT documents_correspondent_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_correspondent
    ADD CONSTRAINT documents_correspondent_unique_name_owner UNIQUE (name, owner_id);

ALTER TABLE ONLY public.documents_customfield
    ADD CONSTRAINT documents_customfield_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_customfield
    ADD CONSTRAINT documents_customfield_unique_name UNIQUE (name);

ALTER TABLE ONLY public.documents_customfieldinstance
    ADD CONSTRAINT documents_customfieldinstance_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_customfieldinstance
    ADD CONSTRAINT documents_customfieldinstance_unique_document_field UNIQUE (document_id, field_id);

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_archive_filename_key UNIQUE (archive_filename);

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_archive_serial_number_key UNIQUE (archive_serial_number);

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_filename_key UNIQUE (filename);

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_document_tags
    ADD CONSTRAINT documents_document_tags_document_id_tag_id_07e90502_uniq UNIQUE (document_id, tag_id);

ALTER TABLE ONLY public.documents_document_tags
    ADD CONSTRAINT documents_document_tags_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_documentbarcode
    ADD CONSTRAINT documents_documentbarcode_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_documenttype
    ADD CONSTRAINT documents_documenttype_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_documenttype
    ADD CONSTRAINT documents_documenttype_unique_name_owner UNIQUE (name, owner_id);

ALTER TABLE ONLY public.documents_note
    ADD CONSTRAINT documents_note_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_paperlesstask
    ADD CONSTRAINT documents_paperlesstask_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_paperlesstask
    ADD CONSTRAINT documents_paperlesstask_task_id_key UNIQUE (task_id);

ALTER TABLE ONLY public.documents_savedview
    ADD CONSTRAINT documents_savedview_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_savedviewfilterrule
    ADD CONSTRAINT documents_savedviewfilterrule_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_sharelink
    ADD CONSTRAINT documents_sharelink_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_sharelink
    ADD CONSTRAINT documents_sharelink_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.documents_sharelinkbundle_documents
    ADD CONSTRAINT documents_sharelinkbundl_sharelinkbundle_id_docum_0213b1dd_uniq UNIQUE (sharelinkbundle_id, document_id);

ALTER TABLE ONLY public.documents_sharelinkbundle_documents
    ADD CONSTRAINT documents_sharelinkbundle_documents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_sharelinkbundle
    ADD CONSTRAINT documents_sharelinkbundle_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_sharelinkbundle
    ADD CONSTRAINT documents_sharelinkbundle_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.documents_storagepath
    ADD CONSTRAINT documents_storagepath_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_storagepath
    ADD CONSTRAINT documents_storagepath_unique_name_owner UNIQUE (name, owner_id);

ALTER TABLE ONLY public.documents_tag
    ADD CONSTRAINT documents_tag_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_tag
    ADD CONSTRAINT documents_tag_unique_name_owner UNIQUE (name, owner_id);

ALTER TABLE ONLY public.documents_uisettings
    ADD CONSTRAINT documents_uisettings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_uisettings
    ADD CONSTRAINT documents_uisettings_user_id_key UNIQUE (user_id);

ALTER TABLE ONLY public.documents_workflow_actions
    ADD CONSTRAINT documents_workflow_actio_workflow_id_workflowacti_314baf08_uniq UNIQUE (workflow_id, workflowaction_id);

ALTER TABLE ONLY public.documents_workflow_actions
    ADD CONSTRAINT documents_workflow_actions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflow
    ADD CONSTRAINT documents_workflow_name_key UNIQUE (name);

ALTER TABLE ONLY public.documents_workflow
    ADD CONSTRAINT documents_workflow_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflow_triggers
    ADD CONSTRAINT documents_workflow_trigg_workflow_id_workflowtrig_379ec41b_uniq UNIQUE (workflow_id, workflowtrigger_id);

ALTER TABLE ONLY public.documents_workflow_triggers
    ADD CONSTRAINT documents_workflow_triggers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_assign_change_groups
    ADD CONSTRAINT documents_workflowaction_assign_change_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_assign_change_users
    ADD CONSTRAINT documents_workflowaction_assign_change_users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_assign_custom_fields
    ADD CONSTRAINT documents_workflowaction_assign_custom_fields_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_assign_tags
    ADD CONSTRAINT documents_workflowaction_assign_tags_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_assign_view_groups
    ADD CONSTRAINT documents_workflowaction_assign_view_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_assign_view_users
    ADD CONSTRAINT documents_workflowaction_assign_view_users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction
    ADD CONSTRAINT documents_workflowaction_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_change_groups
    ADD CONSTRAINT documents_workflowaction_remove_change_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_change_users
    ADD CONSTRAINT documents_workflowaction_remove_change_users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_correspondents
    ADD CONSTRAINT documents_workflowaction_remove_correspondents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_custom_fields
    ADD CONSTRAINT documents_workflowaction_remove_custom_fields_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_document_types
    ADD CONSTRAINT documents_workflowaction_remove_document_types_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_owners
    ADD CONSTRAINT documents_workflowaction_remove_owners_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_storage_paths
    ADD CONSTRAINT documents_workflowaction_remove_storage_paths_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_tags
    ADD CONSTRAINT documents_workflowaction_remove_tags_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_view_groups
    ADD CONSTRAINT documents_workflowaction_remove_view_groups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_view_users
    ADD CONSTRAINT documents_workflowaction_remove_view_users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowaction_remove_correspondents
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_corres_1f286308_uniq UNIQUE (workflowaction_id, correspondent_id);

ALTER TABLE ONLY public.documents_workflowaction_assign_custom_fields
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_custom_7325b36e_uniq UNIQUE (workflowaction_id, customfield_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_custom_fields
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_custom_ae47c643_uniq UNIQUE (workflowaction_id, customfield_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_document_types
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_docume_50d4bf5f_uniq UNIQUE (workflowaction_id, documenttype_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_change_groups
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_group__4103392d_uniq UNIQUE (workflowaction_id, group_id);

ALTER TABLE ONLY public.documents_workflowaction_assign_change_groups
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_group__45663b3e_uniq UNIQUE (workflowaction_id, group_id);

ALTER TABLE ONLY public.documents_workflowaction_assign_view_groups
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_group__7418d9f8_uniq UNIQUE (workflowaction_id, group_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_view_groups
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_group__84d7de00_uniq UNIQUE (workflowaction_id, group_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_storage_paths
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_storag_6db77de5_uniq UNIQUE (workflowaction_id, storagepath_id);

ALTER TABLE ONLY public.documents_workflowaction_assign_tags
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_tag_id_0d903a8d_uniq UNIQUE (workflowaction_id, tag_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_tags
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_tag_id_e1afdcb9_uniq UNIQUE (workflowaction_id, tag_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_owners
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_user_i_2a14ca1c_uniq UNIQUE (workflowaction_id, user_id);

ALTER TABLE ONLY public.documents_workflowaction_assign_change_users
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_user_i_3c0995e7_uniq UNIQUE (workflowaction_id, user_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_view_users
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_user_i_73b4501a_uniq UNIQUE (workflowaction_id, user_id);

ALTER TABLE ONLY public.documents_workflowaction_assign_view_users
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_user_i_8765917e_uniq UNIQUE (workflowaction_id, user_id);

ALTER TABLE ONLY public.documents_workflowaction_remove_change_users
    ADD CONSTRAINT documents_workflowaction_workflowaction_id_user_i_941e6bd9_uniq UNIQUE (workflowaction_id, user_id);

ALTER TABLE ONLY public.documents_workflowactionemail
    ADD CONSTRAINT documents_workflowactionemail_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowactionwebhook
    ADD CONSTRAINT documents_workflowactionwebhook_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowrun
    ADD CONSTRAINT documents_workflowrun_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_correspondents
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_corre_8ef9cd55_uniq UNIQUE (workflowtrigger_id, correspondent_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_correspondents
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_corre_e29ccd76_uniq UNIQUE (workflowtrigger_id, correspondent_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_document_types
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_docum_34054bbd_uniq UNIQUE (workflowtrigger_id, documenttype_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_document_types
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_docum_bdfd1b51_uniq UNIQUE (workflowtrigger_id, documenttype_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_storage_paths
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_stora_55a2c0ef_uniq UNIQUE (workflowtrigger_id, storagepath_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_storage_paths
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_stora_da181eda_uniq UNIQUE (workflowtrigger_id, storagepath_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_all_tags
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_tag_i_6e2b6462_uniq UNIQUE (workflowtrigger_id, tag_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_tags
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_tag_i_af9e82bb_uniq UNIQUE (workflowtrigger_id, tag_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_tags
    ADD CONSTRAINT documents_workflowtrigge_workflowtrigger_id_tag_i_dbb02540_uniq UNIQUE (workflowtrigger_id, tag_id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_all_tags
    ADD CONSTRAINT documents_workflowtrigger_filter_has_all_tags_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_correspondents
    ADD CONSTRAINT documents_workflowtrigger_filter_has_any_correspondents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_document_types
    ADD CONSTRAINT documents_workflowtrigger_filter_has_any_document_types_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_storage_paths
    ADD CONSTRAINT documents_workflowtrigger_filter_has_any_storage_paths_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_correspondents
    ADD CONSTRAINT documents_workflowtrigger_filter_has_not_correspondents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_document_types
    ADD CONSTRAINT documents_workflowtrigger_filter_has_not_document_types_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_storage_paths
    ADD CONSTRAINT documents_workflowtrigger_filter_has_not_storage_paths_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_tags
    ADD CONSTRAINT documents_workflowtrigger_filter_has_not_tags_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_tags
    ADD CONSTRAINT documents_workflowtrigger_filter_has_tags_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.documents_workflowtrigger
    ADD CONSTRAINT documents_workflowtrigger_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.guardian_groupobjectpermission
    ADD CONSTRAINT guardian_groupobjectperm_group_id_permission_id_o_3f189f7c_uniq UNIQUE (group_id, permission_id, object_pk);

ALTER TABLE ONLY public.guardian_groupobjectpermission
    ADD CONSTRAINT guardian_groupobjectpermission_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.guardian_userobjectpermission
    ADD CONSTRAINT guardian_userobjectpermi_user_id_permission_id_ob_b0b3d2fc_uniq UNIQUE (user_id, permission_id, object_pk);

ALTER TABLE ONLY public.guardian_userobjectpermission
    ADD CONSTRAINT guardian_userobjectpermission_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mfa_authenticator
    ADD CONSTRAINT mfa_authenticator_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.paperless_applicationconfiguration
    ADD CONSTRAINT paperless_applicationconfiguration_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.paperless_mail_mailaccount
    ADD CONSTRAINT paperless_mail_mailaccount_name_key UNIQUE (name);

ALTER TABLE ONLY public.paperless_mail_mailaccount
    ADD CONSTRAINT paperless_mail_mailaccount_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.paperless_mail_mailrule_assign_tags
    ADD CONSTRAINT paperless_mail_mailrule__mailrule_id_tag_id_0960fdac_uniq UNIQUE (mailrule_id, tag_id);

ALTER TABLE ONLY public.paperless_mail_mailrule_assign_tags
    ADD CONSTRAINT paperless_mail_mailrule_assign_tags_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.paperless_mail_mailrule
    ADD CONSTRAINT paperless_mail_mailrule_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.paperless_mail_mailrule
    ADD CONSTRAINT paperless_mail_mailrule_unique_name_owner UNIQUE (name, owner_id);

ALTER TABLE ONLY public.paperless_mail_processedmail
    ADD CONSTRAINT paperless_mail_processedmail_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.socialaccount_socialaccount
    ADD CONSTRAINT socialaccount_socialaccount_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.socialaccount_socialaccount
    ADD CONSTRAINT socialaccount_socialaccount_provider_uid_fc810c6e_uniq UNIQUE (provider, uid);

ALTER TABLE ONLY public.socialaccount_socialapp
    ADD CONSTRAINT socialaccount_socialapp_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.socialaccount_socialtoken
    ADD CONSTRAINT socialaccount_socialtoken_app_id_account_id_fca4e0ac_uniq UNIQUE (app_id, account_id);

ALTER TABLE ONLY public.socialaccount_socialtoken
    ADD CONSTRAINT socialaccount_socialtoken_pkey PRIMARY KEY (id);

CREATE INDEX account_emailaddress_email_03be32b2 ON public.account_emailaddress USING btree (email);

CREATE INDEX account_emailaddress_email_03be32b2_like ON public.account_emailaddress USING btree (email varchar_pattern_ops);

CREATE INDEX account_emailaddress_user_id_2c513194 ON public.account_emailaddress USING btree (user_id);

CREATE INDEX account_emailconfirmation_email_address_id_5b7f8c58 ON public.account_emailconfirmation USING btree (email_address_id);

CREATE INDEX account_emailconfirmation_key_f43612bd_like ON public.account_emailconfirmation USING btree (key varchar_pattern_ops);

CREATE INDEX auditlog_logentry_action_229afe39 ON public.auditlog_logentry USING btree (action);

CREATE INDEX auditlog_logentry_actor_id_959271d2 ON public.auditlog_logentry USING btree (actor_id);

CREATE INDEX auditlog_logentry_cid_9f467263 ON public.auditlog_logentry USING btree (cid);

CREATE INDEX auditlog_logentry_cid_9f467263_like ON public.auditlog_logentry USING btree (cid varchar_pattern_ops);

CREATE INDEX auditlog_logentry_content_type_id_75830218 ON public.auditlog_logentry USING btree (content_type_id);

CREATE INDEX auditlog_logentry_object_id_09c2eee8 ON public.auditlog_logentry USING btree (object_id);

CREATE INDEX auditlog_logentry_object_pk_6e3219c0 ON public.auditlog_logentry USING btree (object_pk);

CREATE INDEX auditlog_logentry_object_pk_6e3219c0_like ON public.auditlog_logentry USING btree (object_pk varchar_pattern_ops);

CREATE INDEX auditlog_logentry_timestamp_37867bb0 ON public.auditlog_logentry USING btree ("timestamp");

CREATE INDEX auth_group_name_a6ea08ec_like ON public.auth_group USING btree (name varchar_pattern_ops);

CREATE INDEX auth_group_permissions_group_id_b120cbf9 ON public.auth_group_permissions USING btree (group_id);

CREATE INDEX auth_group_permissions_permission_id_84c5c92e ON public.auth_group_permissions USING btree (permission_id);

CREATE INDEX auth_permission_content_type_id_2f476e4b ON public.auth_permission USING btree (content_type_id);

CREATE INDEX auth_user_groups_group_id_97559544 ON public.auth_user_groups USING btree (group_id);

CREATE INDEX auth_user_groups_user_id_6a12ed8b ON public.auth_user_groups USING btree (user_id);

CREATE INDEX auth_user_user_permissions_permission_id_1fbb5f2c ON public.auth_user_user_permissions USING btree (permission_id);

CREATE INDEX auth_user_user_permissions_user_id_a95ead1b ON public.auth_user_user_permissions USING btree (user_id);

CREATE INDEX auth_user_username_6821ab7c_like ON public.auth_user USING btree (username varchar_pattern_ops);

CREATE INDEX authtoken_token_key_10f0b77e_like ON public.authtoken_token USING btree (key varchar_pattern_ops);

CREATE INDEX django_admin_log_content_type_id_c4bce8eb ON public.django_admin_log USING btree (content_type_id);

CREATE INDEX django_admin_log_user_id_c564eba6 ON public.django_admin_log USING btree (user_id);

CREATE INDEX django_session_expire_date_a5c62663 ON public.django_session USING btree (expire_date);

CREATE INDEX django_session_session_key_c0390e0f_like ON public.django_session USING btree (session_key varchar_pattern_ops);

CREATE INDEX documents_c_field_i_30b51d_idx ON public.documents_customfieldinstance USING btree (field_id, value_date);

CREATE INDEX documents_c_field_i_53ce4e_idx ON public.documents_customfieldinstance USING btree (field_id, value_monetary_amount);

CREATE INDEX documents_c_field_i_c35174_idx ON public.documents_customfieldinstance USING btree (field_id, value_float);

CREATE INDEX documents_c_field_i_d25ab0_idx ON public.documents_customfieldinstance USING btree (field_id, value_int);

CREATE UNIQUE INDEX documents_correspondent_name_uniq ON public.documents_correspondent USING btree (name) WHERE (owner_id IS NULL);

CREATE INDEX documents_correspondent_owner_id_078f7f8a ON public.documents_correspondent USING btree (owner_id);

CREATE INDEX documents_customfield_created_501ef047 ON public.documents_customfield USING btree (created);

CREATE INDEX documents_customfieldinstance_created_75f17f1d ON public.documents_customfieldinstance USING btree (created);

CREATE INDEX documents_customfieldinstance_document_id_610a968e ON public.documents_customfieldinstance USING btree (document_id);

CREATE INDEX documents_customfieldinstance_field_id_6c59e32f ON public.documents_customfieldinstance USING btree (field_id);

CREATE INDEX documents_d_owner_i_ec6ab3_idx ON public.documents_document USING btree (owner_id, created);

CREATE INDEX documents_document_added_28cfa360 ON public.documents_document USING btree (added);

CREATE INDEX documents_document_archive_checksum_cb3e0ec8 ON public.documents_document USING btree (archive_checksum);

CREATE INDEX documents_document_archive_checksum_cb3e0ec8_like ON public.documents_document USING btree (archive_checksum varchar_pattern_ops);

CREATE INDEX documents_document_archive_filename_fc3cb023_like ON public.documents_document USING btree (archive_filename varchar_pattern_ops);

CREATE INDEX documents_document_checksum_75209391 ON public.documents_document USING btree (checksum);

CREATE INDEX documents_document_checksum_75209391_like ON public.documents_document USING btree (checksum varchar_pattern_ops);

CREATE INDEX documents_document_correspondent_id_6164eb0c ON public.documents_document USING btree (correspondent_id);

CREATE INDEX documents_document_created_bedd0818 ON public.documents_document USING btree (created);

CREATE INDEX documents_document_document_type_id_1f88b50c ON public.documents_document USING btree (document_type_id);

CREATE INDEX documents_document_filename_3ee5ad97_like ON public.documents_document USING btree (filename varchar_pattern_ops);

CREATE INDEX documents_document_modified_2eae15bc ON public.documents_document USING btree (modified);

CREATE INDEX documents_document_owner_id_04d2b723 ON public.documents_document USING btree (owner_id);

CREATE INDEX documents_document_page_count_a5279e92 ON public.documents_document USING btree (page_count);

CREATE INDEX documents_document_root_document_id_78094bf2 ON public.documents_document USING btree (root_document_id);

CREATE UNIQUE INDEX documents_document_root_version_index_uniq ON public.documents_document USING btree (root_document_id, version_index) WHERE ((root_document_id IS NOT NULL) AND (version_index IS NOT NULL));

CREATE INDEX documents_document_storage_path_id_07d27bdb ON public.documents_document USING btree (storage_path_id);

CREATE INDEX documents_document_tags_document_id_006f35a8 ON public.documents_document_tags USING btree (document_id);

CREATE INDEX documents_document_tags_tag_id_4f4a71e9 ON public.documents_document_tags USING btree (tag_id);

CREATE INDEX documents_document_title_6b08e02a ON public.documents_document USING btree (title);

CREATE INDEX documents_document_title_6b08e02a_like ON public.documents_document USING btree (title varchar_pattern_ops);

CREATE INDEX documents_document_version_index_febcbe7b ON public.documents_document USING btree (version_index);

CREATE INDEX documents_documentbarcode_document_id_c0313d5b ON public.documents_documentbarcode USING btree (document_id);

CREATE UNIQUE INDEX documents_documenttype_name_uniq ON public.documents_documenttype USING btree (name) WHERE (owner_id IS NULL);

CREATE INDEX documents_documenttype_owner_id_a19f201d ON public.documents_documenttype USING btree (owner_id);

CREATE INDEX documents_note_created_398ee640 ON public.documents_note USING btree (created);

CREATE INDEX documents_note_document_id_ef90a624 ON public.documents_note USING btree (document_id);

CREATE INDEX documents_note_user_id_cab59bae ON public.documents_note USING btree (user_id);

CREATE INDEX documents_p_owner_i_62c545_idx ON public.documents_paperlesstask USING btree (owner_id, acknowledged, date_created);

CREATE INDEX documents_p_status_8aa687_idx ON public.documents_paperlesstask USING btree (status, date_created);

CREATE INDEX documents_p_task_ty_e4a93f_idx ON public.documents_paperlesstask USING btree (task_type, status);

CREATE INDEX documents_paperlesstask_acknowledged_d75e68ba ON public.documents_paperlesstask USING btree (acknowledged);

CREATE INDEX documents_paperlesstask_date_created_90b15f1f ON public.documents_paperlesstask USING btree (date_created);

CREATE INDEX documents_paperlesstask_date_done_d2df1951 ON public.documents_paperlesstask USING btree (date_done);

CREATE INDEX documents_paperlesstask_owner_id_e4512f84 ON public.documents_paperlesstask USING btree (owner_id);

CREATE INDEX documents_paperlesstask_status_6748fa6c ON public.documents_paperlesstask USING btree (status);

CREATE INDEX documents_paperlesstask_status_6748fa6c_like ON public.documents_paperlesstask USING btree (status varchar_pattern_ops);

CREATE INDEX documents_paperlesstask_task_id_24b393f9_like ON public.documents_paperlesstask USING btree (task_id varchar_pattern_ops);

CREATE INDEX documents_paperlesstask_task_type_8802cd7f ON public.documents_paperlesstask USING btree (task_type);

CREATE INDEX documents_paperlesstask_task_type_8802cd7f_like ON public.documents_paperlesstask USING btree (task_type varchar_pattern_ops);

CREATE INDEX documents_paperlesstask_trigger_source_71f17c67 ON public.documents_paperlesstask USING btree (trigger_source);

CREATE INDEX documents_paperlesstask_trigger_source_71f17c67_like ON public.documents_paperlesstask USING btree (trigger_source varchar_pattern_ops);

CREATE INDEX documents_savedview_owner_id_2df222ee ON public.documents_savedview USING btree (owner_id);

CREATE INDEX documents_savedviewfilterrule_saved_view_id_038e2c89 ON public.documents_savedviewfilterrule USING btree (saved_view_id);

CREATE INDEX documents_sharelink_created_23f24996 ON public.documents_sharelink USING btree (created);

CREATE INDEX documents_sharelink_document_id_1be5ab03 ON public.documents_sharelink USING btree (document_id);

CREATE INDEX documents_sharelink_expiration_152b32a7 ON public.documents_sharelink USING btree (expiration);

CREATE INDEX documents_sharelink_owner_id_3fd35a08 ON public.documents_sharelink USING btree (owner_id);

CREATE INDEX documents_sharelink_slug_47d0454d_like ON public.documents_sharelink USING btree (slug varchar_pattern_ops);

CREATE INDEX documents_sharelinkbundle_created_34f0eb12 ON public.documents_sharelinkbundle USING btree (created);

CREATE INDEX documents_sharelinkbundle_documents_document_id_c3012fd7 ON public.documents_sharelinkbundle_documents USING btree (document_id);

CREATE INDEX documents_sharelinkbundle_documents_sharelinkbundle_id_7eca8f52 ON public.documents_sharelinkbundle_documents USING btree (sharelinkbundle_id);

CREATE INDEX documents_sharelinkbundle_expiration_158994e9 ON public.documents_sharelinkbundle USING btree (expiration);

CREATE INDEX documents_sharelinkbundle_owner_id_9138d0af ON public.documents_sharelinkbundle USING btree (owner_id);

CREATE INDEX documents_sharelinkbundle_slug_0c18b39d_like ON public.documents_sharelinkbundle USING btree (slug varchar_pattern_ops);

CREATE UNIQUE INDEX documents_storagepath_name_uniq ON public.documents_storagepath USING btree (name) WHERE (owner_id IS NULL);

CREATE INDEX documents_storagepath_owner_id_c8f01fe8 ON public.documents_storagepath USING btree (owner_id);

CREATE UNIQUE INDEX documents_tag_name_uniq ON public.documents_tag USING btree (name) WHERE (owner_id IS NULL);

CREATE INDEX documents_tag_owner_id_f0875d77 ON public.documents_tag USING btree (owner_id);

CREATE INDEX documents_tag_tn_parent_id_59df9f7f ON public.documents_tag USING btree (tn_parent_id);

CREATE INDEX documents_workflow_actions_workflow_id_5445f1de ON public.documents_workflow_actions USING btree (workflow_id);

CREATE INDEX documents_workflow_actions_workflowaction_id_7ed6a4fa ON public.documents_workflow_actions USING btree (workflowaction_id);

CREATE INDEX documents_workflow_name_e88f7b19_like ON public.documents_workflow USING btree (name varchar_pattern_ops);

CREATE INDEX documents_workflow_triggers_workflow_id_77d63852 ON public.documents_workflow_triggers USING btree (workflow_id);

CREATE INDEX documents_workflow_triggers_workflowtrigger_id_566f2a31 ON public.documents_workflow_triggers USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowaction_a_customfield_id_09df9cc4 ON public.documents_workflowaction_assign_custom_fields USING btree (customfield_id);

CREATE INDEX documents_workflowaction_a_workflowaction_id_03784a7e ON public.documents_workflowaction_assign_change_groups USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_a_workflowaction_id_518e70e5 ON public.documents_workflowaction_assign_custom_fields USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_a_workflowaction_id_79b5b0b4 ON public.documents_workflowaction_assign_view_users USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_a_workflowaction_id_bcc0465a ON public.documents_workflowaction_assign_change_users USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_a_workflowaction_id_f8ef83c1 ON public.documents_workflowaction_assign_view_groups USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_assign_change_groups_group_id_a6d3bc2c ON public.documents_workflowaction_assign_change_groups USING btree (group_id);

CREATE INDEX documents_workflowaction_assign_change_users_user_id_73bec49c ON public.documents_workflowaction_assign_change_users USING btree (user_id);

CREATE INDEX documents_workflowaction_assign_correspondent_id_2292447e ON public.documents_workflowaction USING btree (assign_correspondent_id);

CREATE INDEX documents_workflowaction_assign_document_type_id_4d37bd10 ON public.documents_workflowaction USING btree (assign_document_type_id);

CREATE INDEX documents_workflowaction_assign_owner_id_a409d275 ON public.documents_workflowaction USING btree (assign_owner_id);

CREATE INDEX documents_workflowaction_assign_storage_path_id_6fec4231 ON public.documents_workflowaction USING btree (assign_storage_path_id);

CREATE INDEX documents_workflowaction_assign_tags_tag_id_6770cb84 ON public.documents_workflowaction_assign_tags USING btree (tag_id);

CREATE INDEX documents_workflowaction_assign_tags_workflowaction_id_cc9ef2da ON public.documents_workflowaction_assign_tags USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_assign_view_groups_group_id_0ff4e812 ON public.documents_workflowaction_assign_view_groups USING btree (group_id);

CREATE INDEX documents_workflowaction_assign_view_users_user_id_24b6d448 ON public.documents_workflowaction_assign_view_users USING btree (user_id);

CREATE INDEX documents_workflowaction_email_id_3b59174d ON public.documents_workflowaction USING btree (email_id);

CREATE INDEX documents_workflowaction_r_correspondent_id_16093a7c ON public.documents_workflowaction_remove_correspondents USING btree (correspondent_id);

CREATE INDEX documents_workflowaction_r_customfield_id_80028fe8 ON public.documents_workflowaction_remove_custom_fields USING btree (customfield_id);

CREATE INDEX documents_workflowaction_r_documenttype_id_f99ac948 ON public.documents_workflowaction_remove_document_types USING btree (documenttype_id);

CREATE INDEX documents_workflowaction_r_storagepath_id_3f0130af ON public.documents_workflowaction_remove_storage_paths USING btree (storagepath_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_1b269578 ON public.documents_workflowaction_remove_storage_paths USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_27560d24 ON public.documents_workflowaction_remove_owners USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_4aa554c0 ON public.documents_workflowaction_remove_view_users USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_4ec66e2f ON public.documents_workflowaction_remove_correspondents USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_6629046f ON public.documents_workflowaction_remove_change_groups USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_8c532b6c ON public.documents_workflowaction_remove_view_groups USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_beae1e27 ON public.documents_workflowaction_remove_change_users USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_d80bd370 ON public.documents_workflowaction_remove_document_types USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_r_workflowaction_id_edf3a9f1 ON public.documents_workflowaction_remove_custom_fields USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_remove_change_groups_group_id_905a1a61 ON public.documents_workflowaction_remove_change_groups USING btree (group_id);

CREATE INDEX documents_workflowaction_remove_change_users_user_id_ff94707a ON public.documents_workflowaction_remove_change_users USING btree (user_id);

CREATE INDEX documents_workflowaction_remove_owners_user_id_b9cd308d ON public.documents_workflowaction_remove_owners USING btree (user_id);

CREATE INDEX documents_workflowaction_remove_tags_tag_id_981465ff ON public.documents_workflowaction_remove_tags USING btree (tag_id);

CREATE INDEX documents_workflowaction_remove_tags_workflowaction_id_921c52c0 ON public.documents_workflowaction_remove_tags USING btree (workflowaction_id);

CREATE INDEX documents_workflowaction_remove_view_groups_group_id_43f07529 ON public.documents_workflowaction_remove_view_groups USING btree (group_id);

CREATE INDEX documents_workflowaction_remove_view_users_user_id_1939732e ON public.documents_workflowaction_remove_view_users USING btree (user_id);

CREATE INDEX documents_workflowaction_webhook_id_1011297c ON public.documents_workflowaction USING btree (webhook_id);

CREATE INDEX documents_workflowrun_document_id_dc6f429b ON public.documents_workflowrun USING btree (document_id);

CREATE INDEX documents_workflowrun_run_at_7b149df1 ON public.documents_workflowrun USING btree (run_at);

CREATE INDEX documents_workflowrun_workflow_id_bec88108 ON public.documents_workflowrun USING btree (workflow_id);

CREATE INDEX documents_workflowtrigger__correspondent_id_ac4a7d73 ON public.documents_workflowtrigger_filter_has_not_correspondents USING btree (correspondent_id);

CREATE INDEX documents_workflowtrigger__correspondent_id_e2f177b2 ON public.documents_workflowtrigger_filter_has_any_correspondents USING btree (correspondent_id);

CREATE INDEX documents_workflowtrigger__documenttype_id_395d37fa ON public.documents_workflowtrigger_filter_has_not_document_types USING btree (documenttype_id);

CREATE INDEX documents_workflowtrigger__documenttype_id_e1418db1 ON public.documents_workflowtrigger_filter_has_any_document_types USING btree (documenttype_id);

CREATE INDEX documents_workflowtrigger__storagepath_id_4f759b0d ON public.documents_workflowtrigger_filter_has_any_storage_paths USING btree (storagepath_id);

CREATE INDEX documents_workflowtrigger__storagepath_id_b49241eb ON public.documents_workflowtrigger_filter_has_not_storage_paths USING btree (storagepath_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_0e4ae5ac ON public.documents_workflowtrigger_filter_has_not_storage_paths USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_4a205aa9 ON public.documents_workflowtrigger_filter_has_any_correspondents USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_5e9a227f ON public.documents_workflowtrigger_filter_has_any_storage_paths USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_844712e1 ON public.documents_workflowtrigger_filter_has_tags USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_8665ee72 ON public.documents_workflowtrigger_filter_has_any_document_types USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_b1a69716 ON public.documents_workflowtrigger_filter_has_not_tags USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_bdf4f6cc ON public.documents_workflowtrigger_filter_has_not_correspondents USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_d0ca143d ON public.documents_workflowtrigger_filter_has_not_document_types USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger__workflowtrigger_id_da8dc5d4 ON public.documents_workflowtrigger_filter_has_all_tags USING btree (workflowtrigger_id);

CREATE INDEX documents_workflowtrigger_filter_has_all_tags_tag_id_de7224c0 ON public.documents_workflowtrigger_filter_has_all_tags USING btree (tag_id);

CREATE INDEX documents_workflowtrigger_filter_has_correspondent_id_cd1d603d ON public.documents_workflowtrigger USING btree (filter_has_correspondent_id);

CREATE INDEX documents_workflowtrigger_filter_has_document_type_id_a7f7f504 ON public.documents_workflowtrigger USING btree (filter_has_document_type_id);

CREATE INDEX documents_workflowtrigger_filter_has_not_tags_tag_id_496459b6 ON public.documents_workflowtrigger_filter_has_not_tags USING btree (tag_id);

CREATE INDEX documents_workflowtrigger_filter_has_storage_path_id_c4d7cab1 ON public.documents_workflowtrigger USING btree (filter_has_storage_path_id);

CREATE INDEX documents_workflowtrigger_filter_has_tags_tag_id_f5d3bce9 ON public.documents_workflowtrigger_filter_has_tags USING btree (tag_id);

CREATE INDEX documents_workflowtrigger_filter_mailrule_id_6e1798b1 ON public.documents_workflowtrigger USING btree (filter_mailrule_id);

CREATE INDEX documents_workflowtrigger_schedule_date_custom_field_450a569a ON public.documents_workflowtrigger USING btree (schedule_date_custom_field_id);

CREATE INDEX guardian_gr_group_i_9e7d12_idx ON public.guardian_groupobjectpermission USING btree (group_id, content_type_id, object_pk);

CREATE INDEX guardian_gr_permiss_83545c_idx ON public.guardian_groupobjectpermission USING btree (permission_id, group_id, content_type_id, object_pk);

CREATE INDEX guardian_groupobjectpermission_content_type_id_7ade36b8 ON public.guardian_groupobjectpermission USING btree (content_type_id);

CREATE INDEX guardian_groupobjectpermission_group_id_4bbbfb62 ON public.guardian_groupobjectpermission USING btree (group_id);

CREATE INDEX guardian_groupobjectpermission_permission_id_36572738 ON public.guardian_groupobjectpermission USING btree (permission_id);

CREATE INDEX guardian_us_permiss_e5749c_idx ON public.guardian_userobjectpermission USING btree (permission_id, user_id, content_type_id, object_pk);

CREATE INDEX guardian_us_user_id_8eae14_idx ON public.guardian_userobjectpermission USING btree (user_id, content_type_id, object_pk);

CREATE INDEX guardian_userobjectpermission_content_type_id_2e892405 ON public.guardian_userobjectpermission USING btree (content_type_id);

CREATE INDEX guardian_userobjectpermission_permission_id_71807bfc ON public.guardian_userobjectpermission USING btree (permission_id);

CREATE INDEX guardian_userobjectpermission_user_id_d5c1e964 ON public.guardian_userobjectpermission USING btree (user_id);

CREATE INDEX mfa_authenticator_user_id_0c3a50c0 ON public.mfa_authenticator USING btree (user_id);

CREATE INDEX paperless_mail_mailaccount_name_6af0ff82_like ON public.paperless_mail_mailaccount USING btree (name varchar_pattern_ops);

CREATE INDEX paperless_mail_mailaccount_owner_id_a894e5ee ON public.paperless_mail_mailaccount USING btree (owner_id);

CREATE INDEX paperless_mail_mailrule_account_id_461903f4 ON public.paperless_mail_mailrule USING btree (account_id);

CREATE INDEX paperless_mail_mailrule_assign_correspondent_id_d2651486 ON public.paperless_mail_mailrule USING btree (assign_correspondent_id);

CREATE INDEX paperless_mail_mailrule_assign_document_type_id_cb809476 ON public.paperless_mail_mailrule USING btree (assign_document_type_id);

CREATE INDEX paperless_mail_mailrule_assign_tags_mailrule_id_80d0b16c ON public.paperless_mail_mailrule_assign_tags USING btree (mailrule_id);

CREATE INDEX paperless_mail_mailrule_assign_tags_tag_id_4d0ac8d5 ON public.paperless_mail_mailrule_assign_tags USING btree (tag_id);

CREATE UNIQUE INDEX paperless_mail_mailrule_name_unique ON public.paperless_mail_mailrule USING btree (name) WHERE (owner_id IS NULL);

CREATE INDEX paperless_mail_mailrule_owner_id_05d1c50f ON public.paperless_mail_mailrule USING btree (owner_id);

CREATE INDEX paperless_mail_processedmail_owner_id_63d8ece4 ON public.paperless_mail_processedmail USING btree (owner_id);

CREATE INDEX paperless_mail_processedmail_rule_id_55bb3322 ON public.paperless_mail_processedmail USING btree (rule_id);

CREATE INDEX socialaccount_socialaccount_user_id_8146e70c ON public.socialaccount_socialaccount USING btree (user_id);

CREATE INDEX socialaccount_socialtoken_account_id_951f210e ON public.socialaccount_socialtoken USING btree (account_id);

CREATE INDEX socialaccount_socialtoken_app_id_636a42d7 ON public.socialaccount_socialtoken USING btree (app_id);

CREATE UNIQUE INDEX unique_authenticator_type ON public.mfa_authenticator USING btree (user_id, type) WHERE ((type)::text = ANY ((ARRAY['totp'::character varying, 'recovery_codes'::character varying])::text[]));

CREATE UNIQUE INDEX unique_primary_email ON public.account_emailaddress USING btree (user_id, "primary") WHERE "primary";

CREATE UNIQUE INDEX unique_verified_email ON public.account_emailaddress USING btree (email) WHERE verified;

ALTER TABLE ONLY public.account_emailaddress
    ADD CONSTRAINT account_emailaddress_user_id_2c513194_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.account_emailconfirmation
    ADD CONSTRAINT account_emailconfirm_email_address_id_5b7f8c58_fk_account_e FOREIGN KEY (email_address_id) REFERENCES public.account_emailaddress(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auditlog_logentry
    ADD CONSTRAINT auditlog_logentry_actor_id_959271d2_fk_auth_user_id FOREIGN KEY (actor_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auditlog_logentry
    ADD CONSTRAINT auditlog_logentry_content_type_id_75830218_fk_django_co FOREIGN KEY (content_type_id) REFERENCES public.django_content_type(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auth_group_permissions
    ADD CONSTRAINT auth_group_permissio_permission_id_84c5c92e_fk_auth_perm FOREIGN KEY (permission_id) REFERENCES public.auth_permission(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auth_group_permissions
    ADD CONSTRAINT auth_group_permissions_group_id_b120cbf9_fk_auth_group_id FOREIGN KEY (group_id) REFERENCES public.auth_group(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auth_permission
    ADD CONSTRAINT auth_permission_content_type_id_2f476e4b_fk_django_co FOREIGN KEY (content_type_id) REFERENCES public.django_content_type(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auth_user_groups
    ADD CONSTRAINT auth_user_groups_group_id_97559544_fk_auth_group_id FOREIGN KEY (group_id) REFERENCES public.auth_group(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auth_user_groups
    ADD CONSTRAINT auth_user_groups_user_id_6a12ed8b_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auth_user_user_permissions
    ADD CONSTRAINT auth_user_user_permi_permission_id_1fbb5f2c_fk_auth_perm FOREIGN KEY (permission_id) REFERENCES public.auth_permission(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.auth_user_user_permissions
    ADD CONSTRAINT auth_user_user_permissions_user_id_a95ead1b_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.authtoken_token
    ADD CONSTRAINT authtoken_token_user_id_35299eff_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.django_admin_log
    ADD CONSTRAINT django_admin_log_content_type_id_c4bce8eb_fk_django_co FOREIGN KEY (content_type_id) REFERENCES public.django_content_type(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.django_admin_log
    ADD CONSTRAINT django_admin_log_user_id_c564eba6_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_correspondent
    ADD CONSTRAINT documents_correspondent_owner_id_078f7f8a_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_customfieldinstance
    ADD CONSTRAINT documents_customfiel_document_id_610a968e_fk_documents FOREIGN KEY (document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_customfieldinstance
    ADD CONSTRAINT documents_customfiel_field_id_6c59e32f_fk_documents FOREIGN KEY (field_id) REFERENCES public.documents_customfield(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_correspondent_id_6164eb0c_fk_documents FOREIGN KEY (correspondent_id) REFERENCES public.documents_correspondent(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_document_type_id_1f88b50c_fk_documents FOREIGN KEY (document_type_id) REFERENCES public.documents_documenttype(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_owner_id_04d2b723_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_root_document_id_78094bf2_fk_documents FOREIGN KEY (root_document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_document
    ADD CONSTRAINT documents_document_storage_path_id_07d27bdb_fk_documents FOREIGN KEY (storage_path_id) REFERENCES public.documents_storagepath(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_document_tags
    ADD CONSTRAINT documents_document_t_document_id_006f35a8_fk_documents FOREIGN KEY (document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_document_tags
    ADD CONSTRAINT documents_document_tags_tag_id_4f4a71e9_fk_documents_tag_id FOREIGN KEY (tag_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_documentbarcode
    ADD CONSTRAINT documents_documentba_document_id_c0313d5b_fk_documents FOREIGN KEY (document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_documenttype
    ADD CONSTRAINT documents_documenttype_owner_id_a19f201d_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_note
    ADD CONSTRAINT documents_note_document_id_ef90a624_fk_documents_document_id FOREIGN KEY (document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_note
    ADD CONSTRAINT documents_note_user_id_cab59bae_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_paperlesstask
    ADD CONSTRAINT documents_paperlesstask_owner_id_e4512f84_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_savedview
    ADD CONSTRAINT documents_savedview_owner_id_2df222ee_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_savedviewfilterrule
    ADD CONSTRAINT documents_savedviewf_saved_view_id_038e2c89_fk_documents FOREIGN KEY (saved_view_id) REFERENCES public.documents_savedview(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_sharelink
    ADD CONSTRAINT documents_sharelink_document_id_1be5ab03_fk_documents FOREIGN KEY (document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_sharelink
    ADD CONSTRAINT documents_sharelink_owner_id_3fd35a08_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_sharelinkbundle_documents
    ADD CONSTRAINT documents_sharelinkb_document_id_c3012fd7_fk_documents FOREIGN KEY (document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_sharelinkbundle_documents
    ADD CONSTRAINT documents_sharelinkb_sharelinkbundle_id_7eca8f52_fk_documents FOREIGN KEY (sharelinkbundle_id) REFERENCES public.documents_sharelinkbundle(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_sharelinkbundle
    ADD CONSTRAINT documents_sharelinkbundle_owner_id_9138d0af_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_storagepath
    ADD CONSTRAINT documents_storagepath_owner_id_c8f01fe8_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_tag
    ADD CONSTRAINT documents_tag_owner_id_f0875d77_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_tag
    ADD CONSTRAINT documents_tag_tn_parent_id_59df9f7f_fk_documents_tag_id FOREIGN KEY (tn_parent_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_uisettings
    ADD CONSTRAINT documents_uisettings_user_id_2df2adc2_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflow_actions
    ADD CONSTRAINT documents_workflow_a_workflow_id_5445f1de_fk_documents FOREIGN KEY (workflow_id) REFERENCES public.documents_workflow(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflow_actions
    ADD CONSTRAINT documents_workflow_a_workflowaction_id_7ed6a4fa_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflow_triggers
    ADD CONSTRAINT documents_workflow_t_workflow_id_77d63852_fk_documents FOREIGN KEY (workflow_id) REFERENCES public.documents_workflow(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflow_triggers
    ADD CONSTRAINT documents_workflow_t_workflowtrigger_id_566f2a31_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction
    ADD CONSTRAINT documents_workflowac_assign_correspondent_2292447e_fk_documents FOREIGN KEY (assign_correspondent_id) REFERENCES public.documents_correspondent(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction
    ADD CONSTRAINT documents_workflowac_assign_document_type_4d37bd10_fk_documents FOREIGN KEY (assign_document_type_id) REFERENCES public.documents_documenttype(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction
    ADD CONSTRAINT documents_workflowac_assign_owner_id_a409d275_fk_auth_user FOREIGN KEY (assign_owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction
    ADD CONSTRAINT documents_workflowac_assign_storage_path__6fec4231_fk_documents FOREIGN KEY (assign_storage_path_id) REFERENCES public.documents_storagepath(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_correspondents
    ADD CONSTRAINT documents_workflowac_correspondent_id_16093a7c_fk_documents FOREIGN KEY (correspondent_id) REFERENCES public.documents_correspondent(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_custom_fields
    ADD CONSTRAINT documents_workflowac_customfield_id_09df9cc4_fk_documents FOREIGN KEY (customfield_id) REFERENCES public.documents_customfield(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_custom_fields
    ADD CONSTRAINT documents_workflowac_customfield_id_80028fe8_fk_documents FOREIGN KEY (customfield_id) REFERENCES public.documents_customfield(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_document_types
    ADD CONSTRAINT documents_workflowac_documenttype_id_f99ac948_fk_documents FOREIGN KEY (documenttype_id) REFERENCES public.documents_documenttype(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction
    ADD CONSTRAINT documents_workflowac_email_id_3b59174d_fk_documents FOREIGN KEY (email_id) REFERENCES public.documents_workflowactionemail(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_view_groups
    ADD CONSTRAINT documents_workflowac_group_id_0ff4e812_fk_auth_grou FOREIGN KEY (group_id) REFERENCES public.auth_group(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_view_groups
    ADD CONSTRAINT documents_workflowac_group_id_43f07529_fk_auth_grou FOREIGN KEY (group_id) REFERENCES public.auth_group(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_change_groups
    ADD CONSTRAINT documents_workflowac_group_id_905a1a61_fk_auth_grou FOREIGN KEY (group_id) REFERENCES public.auth_group(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_change_groups
    ADD CONSTRAINT documents_workflowac_group_id_a6d3bc2c_fk_auth_grou FOREIGN KEY (group_id) REFERENCES public.auth_group(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_storage_paths
    ADD CONSTRAINT documents_workflowac_storagepath_id_3f0130af_fk_documents FOREIGN KEY (storagepath_id) REFERENCES public.documents_storagepath(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_tags
    ADD CONSTRAINT documents_workflowac_tag_id_6770cb84_fk_documents FOREIGN KEY (tag_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_tags
    ADD CONSTRAINT documents_workflowac_tag_id_981465ff_fk_documents FOREIGN KEY (tag_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_view_users
    ADD CONSTRAINT documents_workflowac_user_id_1939732e_fk_auth_user FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_view_users
    ADD CONSTRAINT documents_workflowac_user_id_24b6d448_fk_auth_user FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_change_users
    ADD CONSTRAINT documents_workflowac_user_id_73bec49c_fk_auth_user FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_owners
    ADD CONSTRAINT documents_workflowac_user_id_b9cd308d_fk_auth_user FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_change_users
    ADD CONSTRAINT documents_workflowac_user_id_ff94707a_fk_auth_user FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction
    ADD CONSTRAINT documents_workflowac_webhook_id_1011297c_fk_documents FOREIGN KEY (webhook_id) REFERENCES public.documents_workflowactionwebhook(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_change_groups
    ADD CONSTRAINT documents_workflowac_workflowaction_id_03784a7e_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_storage_paths
    ADD CONSTRAINT documents_workflowac_workflowaction_id_1b269578_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_owners
    ADD CONSTRAINT documents_workflowac_workflowaction_id_27560d24_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_view_users
    ADD CONSTRAINT documents_workflowac_workflowaction_id_4aa554c0_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_correspondents
    ADD CONSTRAINT documents_workflowac_workflowaction_id_4ec66e2f_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_custom_fields
    ADD CONSTRAINT documents_workflowac_workflowaction_id_518e70e5_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_change_groups
    ADD CONSTRAINT documents_workflowac_workflowaction_id_6629046f_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_view_users
    ADD CONSTRAINT documents_workflowac_workflowaction_id_79b5b0b4_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_view_groups
    ADD CONSTRAINT documents_workflowac_workflowaction_id_8c532b6c_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_tags
    ADD CONSTRAINT documents_workflowac_workflowaction_id_921c52c0_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_change_users
    ADD CONSTRAINT documents_workflowac_workflowaction_id_bcc0465a_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_change_users
    ADD CONSTRAINT documents_workflowac_workflowaction_id_beae1e27_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_tags
    ADD CONSTRAINT documents_workflowac_workflowaction_id_cc9ef2da_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_document_types
    ADD CONSTRAINT documents_workflowac_workflowaction_id_d80bd370_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_remove_custom_fields
    ADD CONSTRAINT documents_workflowac_workflowaction_id_edf3a9f1_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowaction_assign_view_groups
    ADD CONSTRAINT documents_workflowac_workflowaction_id_f8ef83c1_fk_documents FOREIGN KEY (workflowaction_id) REFERENCES public.documents_workflowaction(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowrun
    ADD CONSTRAINT documents_workflowru_document_id_dc6f429b_fk_documents FOREIGN KEY (document_id) REFERENCES public.documents_document(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowrun
    ADD CONSTRAINT documents_workflowru_workflow_id_bec88108_fk_documents FOREIGN KEY (workflow_id) REFERENCES public.documents_workflow(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_correspondents
    ADD CONSTRAINT documents_workflowtr_correspondent_id_ac4a7d73_fk_documents FOREIGN KEY (correspondent_id) REFERENCES public.documents_correspondent(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_correspondents
    ADD CONSTRAINT documents_workflowtr_correspondent_id_e2f177b2_fk_documents FOREIGN KEY (correspondent_id) REFERENCES public.documents_correspondent(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_document_types
    ADD CONSTRAINT documents_workflowtr_documenttype_id_395d37fa_fk_documents FOREIGN KEY (documenttype_id) REFERENCES public.documents_documenttype(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_document_types
    ADD CONSTRAINT documents_workflowtr_documenttype_id_e1418db1_fk_documents FOREIGN KEY (documenttype_id) REFERENCES public.documents_documenttype(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger
    ADD CONSTRAINT documents_workflowtr_filter_has_correspon_cd1d603d_fk_documents FOREIGN KEY (filter_has_correspondent_id) REFERENCES public.documents_correspondent(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger
    ADD CONSTRAINT documents_workflowtr_filter_has_document__a7f7f504_fk_documents FOREIGN KEY (filter_has_document_type_id) REFERENCES public.documents_documenttype(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger
    ADD CONSTRAINT documents_workflowtr_filter_has_storage_p_c4d7cab1_fk_documents FOREIGN KEY (filter_has_storage_path_id) REFERENCES public.documents_storagepath(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger
    ADD CONSTRAINT documents_workflowtr_filter_mailrule_id_6e1798b1_fk_paperless FOREIGN KEY (filter_mailrule_id) REFERENCES public.paperless_mail_mailrule(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger
    ADD CONSTRAINT documents_workflowtr_schedule_date_custom_450a569a_fk_documents FOREIGN KEY (schedule_date_custom_field_id) REFERENCES public.documents_customfield(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_storage_paths
    ADD CONSTRAINT documents_workflowtr_storagepath_id_4f759b0d_fk_documents FOREIGN KEY (storagepath_id) REFERENCES public.documents_storagepath(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_storage_paths
    ADD CONSTRAINT documents_workflowtr_storagepath_id_b49241eb_fk_documents FOREIGN KEY (storagepath_id) REFERENCES public.documents_storagepath(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_tags
    ADD CONSTRAINT documents_workflowtr_tag_id_496459b6_fk_documents FOREIGN KEY (tag_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_all_tags
    ADD CONSTRAINT documents_workflowtr_tag_id_de7224c0_fk_documents FOREIGN KEY (tag_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_tags
    ADD CONSTRAINT documents_workflowtr_tag_id_f5d3bce9_fk_documents FOREIGN KEY (tag_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_storage_paths
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_0e4ae5ac_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_correspondents
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_4a205aa9_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_storage_paths
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_5e9a227f_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_tags
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_844712e1_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_any_document_types
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_8665ee72_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_tags
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_b1a69716_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_correspondents
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_bdf4f6cc_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_not_document_types
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_d0ca143d_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.documents_workflowtrigger_filter_has_all_tags
    ADD CONSTRAINT documents_workflowtr_workflowtrigger_id_da8dc5d4_fk_documents FOREIGN KEY (workflowtrigger_id) REFERENCES public.documents_workflowtrigger(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.guardian_groupobjectpermission
    ADD CONSTRAINT guardian_groupobject_content_type_id_7ade36b8_fk_django_co FOREIGN KEY (content_type_id) REFERENCES public.django_content_type(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.guardian_groupobjectpermission
    ADD CONSTRAINT guardian_groupobject_group_id_4bbbfb62_fk_auth_grou FOREIGN KEY (group_id) REFERENCES public.auth_group(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.guardian_groupobjectpermission
    ADD CONSTRAINT guardian_groupobject_permission_id_36572738_fk_auth_perm FOREIGN KEY (permission_id) REFERENCES public.auth_permission(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.guardian_userobjectpermission
    ADD CONSTRAINT guardian_userobjectp_content_type_id_2e892405_fk_django_co FOREIGN KEY (content_type_id) REFERENCES public.django_content_type(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.guardian_userobjectpermission
    ADD CONSTRAINT guardian_userobjectp_permission_id_71807bfc_fk_auth_perm FOREIGN KEY (permission_id) REFERENCES public.auth_permission(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.guardian_userobjectpermission
    ADD CONSTRAINT guardian_userobjectpermission_user_id_d5c1e964_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.mfa_authenticator
    ADD CONSTRAINT mfa_authenticator_user_id_0c3a50c0_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_mailaccount
    ADD CONSTRAINT paperless_mail_mailaccount_owner_id_a894e5ee_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_mailrule
    ADD CONSTRAINT paperless_mail_mailr_account_id_461903f4_fk_paperless FOREIGN KEY (account_id) REFERENCES public.paperless_mail_mailaccount(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_mailrule
    ADD CONSTRAINT paperless_mail_mailr_assign_correspondent_d2651486_fk_documents FOREIGN KEY (assign_correspondent_id) REFERENCES public.documents_correspondent(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_mailrule
    ADD CONSTRAINT paperless_mail_mailr_assign_document_type_cb809476_fk_documents FOREIGN KEY (assign_document_type_id) REFERENCES public.documents_documenttype(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_mailrule_assign_tags
    ADD CONSTRAINT paperless_mail_mailr_mailrule_id_80d0b16c_fk_paperless FOREIGN KEY (mailrule_id) REFERENCES public.paperless_mail_mailrule(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_mailrule_assign_tags
    ADD CONSTRAINT paperless_mail_mailr_tag_id_4d0ac8d5_fk_documents FOREIGN KEY (tag_id) REFERENCES public.documents_tag(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_mailrule
    ADD CONSTRAINT paperless_mail_mailrule_owner_id_05d1c50f_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_processedmail
    ADD CONSTRAINT paperless_mail_proce_rule_id_55bb3322_fk_paperless FOREIGN KEY (rule_id) REFERENCES public.paperless_mail_mailrule(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.paperless_mail_processedmail
    ADD CONSTRAINT paperless_mail_processedmail_owner_id_63d8ece4_fk_auth_user_id FOREIGN KEY (owner_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.socialaccount_socialtoken
    ADD CONSTRAINT socialaccount_social_account_id_951f210e_fk_socialacc FOREIGN KEY (account_id) REFERENCES public.socialaccount_socialaccount(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.socialaccount_socialtoken
    ADD CONSTRAINT socialaccount_social_app_id_636a42d7_fk_socialacc FOREIGN KEY (app_id) REFERENCES public.socialaccount_socialapp(id) DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.socialaccount_socialaccount
    ADD CONSTRAINT socialaccount_socialaccount_user_id_8146e70c_fk_auth_user_id FOREIGN KEY (user_id) REFERENCES public.auth_user(id) DEFERRABLE INITIALLY DEFERRED;

